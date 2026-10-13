//! Downloading and launching vanilla Minecraft using Mojang's public metadata.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rayon::prelude::*;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const VERSION_MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const JAVA_RUNTIMES_URL: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const RESOURCES_URL: &str = "https://resources.download.minecraft.net";
const DOWNLOAD_THREADS: usize = 16;

/// Reports a status message and a progress value in `0.0..=1.0`.
pub type Progress<'a> = &'a (dyn Fn(String, f32) + Sync);

// ---------------------------------------------------------------------------
// Metadata types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
}

#[derive(Deserialize)]
struct VersionManifest {
    versions: Vec<VersionEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionJson {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    main_class: String,
    asset_index: AssetIndexRef,
    downloads: VersionDownloads,
    #[serde(default)]
    libraries: Vec<Library>,
    arguments: Option<Arguments>,
    minecraft_arguments: Option<String>,
    java_version: Option<JavaVersion>,
}

#[derive(Deserialize)]
struct AssetIndexRef {
    id: String,
    url: String,
    size: u64,
}

#[derive(Deserialize)]
struct VersionDownloads {
    client: Artifact,
}

#[derive(Deserialize, Clone)]
struct Artifact {
    path: Option<String>,
    url: String,
    size: u64,
}

#[derive(Deserialize)]
struct Library {
    name: String,
    downloads: Option<LibraryDownloads>,
    #[serde(default)]
    rules: Vec<Rule>,
    natives: Option<HashMap<String, String>>,
    extract: Option<Extract>,
}

#[derive(Deserialize)]
struct LibraryDownloads {
    artifact: Option<Artifact>,
    classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Deserialize, Default)]
struct Extract {
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Deserialize)]
struct Rule {
    action: String,
    os: Option<OsRule>,
    features: Option<HashMap<String, bool>>,
}

#[derive(Deserialize)]
struct OsRule {
    name: Option<String>,
    arch: Option<String>,
}

#[derive(Deserialize)]
struct Arguments {
    #[serde(default)]
    game: Vec<Value>,
    #[serde(default)]
    jvm: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JavaVersion {
    component: String,
}

#[derive(Deserialize)]
struct AssetIndex {
    objects: HashMap<String, AssetObject>,
    #[serde(default, rename = "virtual")]
    is_virtual: bool,
    #[serde(default)]
    map_to_resources: bool,
}

#[derive(Deserialize)]
struct AssetObject {
    hash: String,
    size: u64,
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    }
}

fn os_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "x86",
        "aarch64" => "arm64",
        other => other,
    }
}

fn classpath_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

fn java_runtime_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86") => "windows-x86",
        ("windows", "aarch64") => "windows-arm64",
        ("windows", _) => "windows-x64",
        ("macos", "aarch64") => "mac-os-arm64",
        ("macos", _) => "mac-os",
        (_, "x86") => "linux-i386",
        _ => "linux",
    }
}

fn rule_matches(rule: &Rule) -> bool {
    if let Some(os) = &rule.os {
        if os.name.as_deref().is_some_and(|n| n != os_name()) {
            return false;
        }
        if os.arch.as_deref().is_some_and(|a| a != os_arch()) {
            return false;
        }
    }
    // No launcher features (demo mode, custom resolution, quick play, ...) are enabled.
    if let Some(features) = &rule.features {
        if features.values().any(|&required| required) {
            return false;
        }
    }
    true
}

fn rules_allow(rules: &[Rule]) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules.iter().filter(|r| rule_matches(r)) {
        allowed = rule.action == "allow";
    }
    allowed
}

/// Directory holding all launcher data (`%APPDATA%\.mc-launcher` on Windows).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".mc-launcher")
}

// ---------------------------------------------------------------------------
// Networking
// ---------------------------------------------------------------------------

pub fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("mc-launcher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()?)
}

fn get_json<T: DeserializeOwned>(client: &Client, url: &str) -> Result<T> {
    client
        .get(url)
        .send()?
        .error_for_status()?
        .json()
        .with_context(|| format!("parsing {url}"))
}

/// Downloads `url` to `path`, reusing an existing file when its size matches.
fn download(client: &Client, url: &str, path: &Path, size: Option<u64>) -> Result<()> {
    if let Ok(meta) = fs::metadata(path) {
        if size.is_none_or(|s| meta.len() == s) {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);

    let mut last_err = None;
    for _ in 0..3 {
        let attempt = (|| -> Result<()> {
            let mut resp = client.get(url).send()?.error_for_status()?;
            let mut file = File::create(&tmp)?;
            io::copy(&mut resp, &mut file)?;
            Ok(())
        })();
        match attempt {
            Ok(()) => {
                fs::rename(&tmp, path)?;
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
    }
    let _ = fs::remove_file(&tmp);
    Err(last_err.unwrap().context(format!("downloading {url}")))
}

struct Job {
    url: String,
    path: PathBuf,
    size: Option<u64>,
}

fn download_all(client: &Client, jobs: &[Job], label: &str, progress: Progress) -> Result<()> {
    let total = jobs.len().max(1);
    let done = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(DOWNLOAD_THREADS)
        .build()?;
    progress(format!("{label} (0/{})", jobs.len()), 0.0);
    pool.install(|| {
        jobs.par_iter().try_for_each(|job| {
            download(client, &job.url, &job.path, job.size)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 20 == 0 || n == jobs.len() {
                progress(format!("{label} ({n}/{})", jobs.len()), n as f32 / total as f32);
            }
            Ok(())
        })
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn fetch_versions(client: &Client) -> Result<Vec<VersionEntry>> {
    let manifest: VersionManifest = get_json(client, VERSION_MANIFEST_URL)?;
    Ok(manifest.versions)
}

/// Downloads everything `version` needs and starts the game in offline mode.
pub fn launch(client: &Client, version: &VersionEntry, username: &str, progress: Progress) -> Result<Child> {
    let root = data_dir();
    let game_dir = root.join("game");
    let libraries_dir = root.join("libraries");
    let assets_dir = root.join("assets");
    let version_dir = root.join("versions").join(&version.id);
    let natives_dir = version_dir.join("natives");
    fs::create_dir_all(&game_dir)?;

    // Version metadata
    progress(format!("Fetching {} metadata...", version.id), 0.0);
    let json_path = version_dir.join(format!("{}.json", version.id));
    download(client, &version.url, &json_path, None)?;
    let meta: VersionJson = serde_json::from_slice(&fs::read(&json_path)?)
        .with_context(|| format!("parsing {}", json_path.display()))?;

    // Client jar + libraries
    let client_jar = version_dir.join(format!("{}.jar", meta.id));
    let mut jobs = vec![Job {
        url: meta.downloads.client.url.clone(),
        path: client_jar.clone(),
        size: Some(meta.downloads.client.size),
    }];
    let mut classpath = Vec::new();
    let mut seen = HashSet::new();
    let mut natives = Vec::new();

    for lib in meta.libraries.iter().filter(|l| rules_allow(&l.rules)) {
        let Some(downloads) = &lib.downloads else { continue };
        if let Some(artifact) = &downloads.artifact {
            if let Some(rel) = &artifact.path {
                let path = libraries_dir.join(rel);
                if !artifact.url.is_empty() && seen.insert(path.clone()) {
                    jobs.push(Job { url: artifact.url.clone(), path: path.clone(), size: Some(artifact.size) });
                    classpath.push(path);
                }
            }
        }
        // Legacy (pre-1.19) natives shipped as classifier jars that must be extracted.
        let classifier = lib
            .natives
            .as_ref()
            .and_then(|n| n.get(os_name()))
            .map(|c| c.replace("${arch}", if cfg!(target_pointer_width = "64") { "64" } else { "32" }));
        if let (Some(classifier), Some(classifiers)) = (classifier, &downloads.classifiers) {
            match classifiers.get(&classifier) {
                Some(artifact) if artifact.path.is_some() => {
                    let path = libraries_dir.join(artifact.path.as_ref().unwrap());
                    jobs.push(Job { url: artifact.url.clone(), path: path.clone(), size: Some(artifact.size) });
                    let exclude = lib.extract.as_ref().map(|e| e.exclude.clone()).unwrap_or_default();
                    natives.push((path, exclude));
                }
                _ => eprintln!("missing native {classifier} for {}", lib.name),
            }
        }
    }
    classpath.push(client_jar);
    download_all(client, &jobs, "Downloading game files", progress)?;

    progress("Extracting natives...".into(), 1.0);
    fs::create_dir_all(&natives_dir)?;
    for (jar, exclude) in &natives {
        extract_natives(jar, &natives_dir, exclude)?;
    }

    // Assets
    progress("Fetching asset index...".into(), 0.0);
    let index_path = assets_dir.join("indexes").join(format!("{}.json", meta.asset_index.id));
    download(client, &meta.asset_index.url, &index_path, Some(meta.asset_index.size))?;
    let index: AssetIndex = serde_json::from_slice(&fs::read(&index_path)?)?;
    let object_path = |hash: &str| assets_dir.join("objects").join(&hash[..2]).join(hash);
    let asset_jobs: Vec<Job> = index
        .objects
        .values()
        .map(|o| Job {
            url: format!("{RESOURCES_URL}/{}/{}", &o.hash[..2], o.hash),
            path: object_path(&o.hash),
            size: Some(o.size),
        })
        .collect();
    download_all(client, &asset_jobs, "Downloading assets", progress)?;

    // Very old versions read assets by name instead of by hash.
    let legacy_assets_dir = if index.map_to_resources {
        Some(game_dir.join("resources"))
    } else if index.is_virtual {
        Some(assets_dir.join("virtual").join(&meta.asset_index.id))
    } else {
        None
    };
    if let Some(dir) = &legacy_assets_dir {
        progress("Preparing legacy assets...".into(), 1.0);
        for (name, obj) in &index.objects {
            let dest = dir.join(name);
            if !dest.exists() {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(object_path(&obj.hash), &dest)?;
            }
        }
    }

    // Java
    let component = meta
        .java_version
        .as_ref()
        .map_or("jre-legacy", |j| j.component.as_str());
    let java = match ensure_java(client, &root.join("runtime"), component, progress) {
        Ok(java) => java,
        Err(e) => {
            eprintln!("could not provide Java runtime {component}: {e:#}; falling back to system java");
            PathBuf::from("java")
        }
    };

    // Arguments
    let sep = classpath_separator();
    let classpath = classpath
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(sep);
    let (mut jvm_args, game_args) = match (&meta.arguments, &meta.minecraft_arguments) {
        (Some(args), _) => (collect_args(&args.jvm), collect_args(&args.game)),
        (None, Some(legacy)) => (
            vec!["-Djava.library.path=${natives_directory}".into(), "-cp".into(), "${classpath}".into()],
            legacy.split_whitespace().map(String::from).collect(),
        ),
        (None, None) => bail!("version {} has no launch arguments", meta.id),
    };
    jvm_args.insert(0, "-Xmx2G".into());

    let path_str = |p: &Path| p.to_string_lossy().into_owned();
    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", username.to_string()),
        ("version_name", meta.id.clone()),
        ("game_directory", path_str(&game_dir)),
        ("assets_root", path_str(&assets_dir)),
        ("game_assets", path_str(legacy_assets_dir.as_deref().unwrap_or(&assets_dir))),
        ("assets_index_name", meta.asset_index.id.clone()),
        ("auth_uuid", offline_uuid(username)),
        ("auth_access_token", "0".into()),
        ("auth_session", "0".into()),
        ("auth_xuid", "0".into()),
        ("clientid", "0".into()),
        ("user_type", "legacy".into()),
        ("user_properties", "{}".into()),
        ("version_type", meta.kind.clone()),
        ("natives_directory", path_str(&natives_dir)),
        ("library_directory", path_str(&libraries_dir)),
        ("classpath_separator", sep.into()),
        ("classpath", classpath),
        ("launcher_name", "mc-launcher".into()),
        ("launcher_version", env!("CARGO_PKG_VERSION").into()),
    ]);
    let substitute = |arg: &String| {
        let mut out = arg.clone();
        for (key, value) in &vars {
            out = out.replace(&format!("${{{key}}}"), value);
        }
        out
    };

    progress(format!("Starting Minecraft {}...", meta.id), 1.0);
    let logs_dir = root.join("logs");
    fs::create_dir_all(&logs_dir)?;
    let log = File::create(logs_dir.join("latest-launch.log"))?;

    let mut cmd = Command::new(&java);
    cmd.args(jvm_args.iter().map(substitute))
        .arg(&meta.main_class)
        .args(game_args.iter().map(substitute))
        .current_dir(&game_dir)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().with_context(|| format!("starting {}", java.display()))
}

// ---------------------------------------------------------------------------
// Launch helpers
// ---------------------------------------------------------------------------

fn collect_args(values: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        match value {
            Value::String(s) => out.push(s.clone()),
            Value::Object(obj) => {
                let rules: Vec<Rule> = obj
                    .get("rules")
                    .and_then(|r| serde_json::from_value(r.clone()).ok())
                    .unwrap_or_default();
                if !rules_allow(&rules) {
                    continue;
                }
                match obj.get("value") {
                    Some(Value::String(s)) => out.push(s.clone()),
                    Some(Value::Array(items)) => {
                        out.extend(items.iter().filter_map(|v| v.as_str().map(String::from)))
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

fn extract_natives(jar: &Path, dest: &Path, exclude: &[String]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(jar)?)
        .with_context(|| format!("opening {}", jar.display()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name()?.into_owned();
        if entry.is_dir() || exclude.iter().any(|ex| name.starts_with(ex.as_str())) {
            continue;
        }
        let Some(rel) = entry.enclosed_name() else { continue };
        let out_path = dest.join(rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut File::create(&out_path)?)?;
    }
    Ok(())
}

/// Stable per-username UUID so offline worlds keep the same player data.
fn offline_uuid(username: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let half = |salt: u8| {
        let mut h = DefaultHasher::new();
        ("OfflinePlayer:", username, salt).hash(&mut h);
        h.finish()
    };
    let (a, b) = (half(0), half(1));
    let a = (a & !0xF000) | 0x3000; // version 3
    let b = (b & !(0xC000u64 << 48)) | (0x8000u64 << 48); // RFC 4122 variant
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xFFFF,
        a & 0xFFFF,
        b >> 48,
        b & 0xFFFF_FFFF_FFFF
    )
}

/// Downloads Mojang's bundled Java runtime `component` and returns its java executable.
fn ensure_java(client: &Client, runtimes_dir: &Path, component: &str, progress: Progress) -> Result<PathBuf> {
    let base = runtimes_dir.join(component);
    let java = if cfg!(windows) {
        base.join("bin").join("javaw.exe")
    } else if cfg!(target_os = "macos") {
        base.join("jre.bundle/Contents/Home/bin/java")
    } else {
        base.join("bin").join("java")
    };
    let marker = base.join(".complete");
    if marker.exists() && java.exists() {
        return Ok(java);
    }

    progress(format!("Fetching Java runtime {component}..."), 0.0);
    let all: Value = get_json(client, JAVA_RUNTIMES_URL)?;
    let manifest_url = all[java_runtime_platform()][component][0]["manifest"]["url"]
        .as_str()
        .ok_or_else(|| anyhow!("no {component} runtime for {}", java_runtime_platform()))?;
    let manifest: Value = get_json(client, manifest_url)?;
    let files = manifest["files"]
        .as_object()
        .ok_or_else(|| anyhow!("malformed runtime manifest"))?;

    let mut jobs = Vec::new();
    let mut executables = Vec::new();
    for (rel, file) in files {
        let path = base.join(rel);
        match file["type"].as_str() {
            Some("directory") => fs::create_dir_all(&path)?,
            Some("file") => {
                let raw = &file["downloads"]["raw"];
                let url = raw["url"].as_str().ok_or_else(|| anyhow!("no url for {rel}"))?;
                jobs.push(Job { url: url.to_string(), path: path.clone(), size: raw["size"].as_u64() });
                if file["executable"].as_bool() == Some(true) {
                    executables.push(path);
                }
            }
            #[cfg(unix)]
            Some("link") => {
                if let Some(target) = file["target"].as_str() {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let _ = std::os::unix::fs::symlink(target, &path);
                }
            }
            _ => {}
        }
    }
    download_all(client, &jobs, &format!("Downloading Java ({component})"), progress)?;

    #[cfg(unix)]
    for path in &executables {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    let _ = executables;

    File::create(&marker)?;
    Ok(java)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(v: Value) -> Vec<Rule> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn empty_rules_allow() {
        assert!(rules_allow(&[]));
    }

    #[test]
    fn os_rules() {
        let only_here = rules(json!([{ "action": "allow", "os": { "name": os_name() } }]));
        assert!(rules_allow(&only_here));
        let all_but_here = rules(json!([
            { "action": "allow" },
            { "action": "disallow", "os": { "name": os_name() } }
        ]));
        assert!(!rules_allow(&all_but_here));
        let elsewhere = rules(json!([{ "action": "allow", "os": { "name": "nonexistent-os" } }]));
        assert!(!rules_allow(&elsewhere));
    }

    #[test]
    fn feature_rules_are_disabled() {
        let demo = rules(json!([{ "action": "allow", "features": { "is_demo_user": true } }]));
        assert!(!rules_allow(&demo));
    }

    #[test]
    fn collects_modern_arguments() {
        let args = collect_args(&[
            json!("--username"),
            json!("${auth_player_name}"),
            json!({ "rules": [{ "action": "allow", "features": { "has_custom_resolution": true } }],
                    "value": ["--width", "${resolution_width}"] }),
            json!({ "rules": [{ "action": "allow", "os": { "name": os_name() } }],
                    "value": "-Dhere=1" }),
        ]);
        assert_eq!(args, ["--username", "${auth_player_name}", "-Dhere=1"]);
    }

    #[test]
    fn offline_uuid_is_stable_and_well_formed() {
        let id = offline_uuid("Steve");
        assert_eq!(id, offline_uuid("Steve"));
        assert_ne!(id, offline_uuid("Alex"));
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "3");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }
}
