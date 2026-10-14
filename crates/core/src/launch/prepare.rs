//! Making sure every file a version (and its mod loader) needs is on disk.

use std::collections::HashSet;
use std::path::PathBuf;

use super::{LaunchContext, OutputSink};
use super::natives::extract_natives;
use crate::loader::{self, InstallContext, ModLoader};
use crate::meta::assets::asset_jobs;
use crate::meta::rules::{Features, os_name, rules_allow};
use crate::meta::version::{VersionJson, load_version_json};
use crate::net::{Job, download_all};
use crate::{Progress, Result};

pub struct Prepared {
    /// The vanilla version with the loader profile (if any) merged in.
    pub version: VersionJson,
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub java: PathBuf,
}

/// What to prepare: a game version plus an optional mod loader version.
pub struct Target<'a> {
    pub game_version: &'a str,
    pub loader: ModLoader,
    pub loader_version: &'a str,
}

/// On Apple Silicon, whether the version's natives are Intel-only: 1.13-1.18 ship LWJGL 3.1/3.2
/// as `natives-macos` classifier jars with no arm64 build, while 1.19+ (LWJGL 3.3) list
/// `natives-macos-arm64` as a plain library instead.
fn needs_intel_java(version: &VersionJson) -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
        && version.libraries.iter().any(|l| l.natives.as_ref().is_some_and(|n| n.contains_key("osx")))
}

/// Downloads Java (unless `java_path` is set), the loader, client, libraries, natives and assets.
pub fn prepare(
    ctx: &LaunchContext,
    target: &Target,
    java_path: &str,
    threads: usize,
    progress: Progress,
    output: &OutputSink,
) -> Result<Prepared> {
    let paths = ctx.paths;
    let game_version = target.game_version;
    progress(format!("Fetching {game_version} metadata..."), 0.0);
    let entry = ctx.versions.iter().find(|v| v.id == game_version);
    let mut version = load_version_json(ctx.client, paths, game_version, entry)?;

    let java = if !java_path.trim().is_empty() {
        PathBuf::from(java_path.trim())
    } else if let Some(jv) = &version.java_version {
        if needs_intel_java(&version) {
            crate::java::ensure_intel_mac_runtime(ctx.client, &paths.runtime_dir(), &jv.component, threads, progress)?
        } else {
            crate::java::ensure_runtime(ctx.client, &paths.runtime_dir(), &jv.component, threads, progress)?
        }
    } else {
        PathBuf::from("java")
    };

    if target.loader.is_modded() {
        let install = InstallContext { client: ctx.client, paths, java: &java, progress, output };
        if let Some(profile) = loader::install(&install, target.loader, game_version, target.loader_version)? {
            version.apply_loader(profile);
        }
    }

    let version_dir = paths.version_dir(&version.id);
    let libraries_dir = paths.libraries_dir();
    let client_jar = version_dir.join(format!("{}.jar", version.id));
    let mut jobs = vec![Job {
        url: version.downloads.client.url.clone(),
        path: client_jar.clone(),
        size: Some(version.downloads.client.size),
    }];
    let mut classpath = Vec::new();
    let mut seen = HashSet::new();
    let mut natives = Vec::new();

    let features = Features::default();
    for lib in version.libraries.iter().filter(|l| rules_allow(&l.rules, &features)) {
        if let Some(artifact) = lib.artifact() {
            if let Some(rel) = &artifact.path {
                let path = libraries_dir.join(rel);
                if seen.insert(path.clone()) {
                    // An empty URL means the file is generated locally (e.g. by the NeoForge installer).
                    if !artifact.url.is_empty() {
                        let size = (artifact.size > 0).then_some(artifact.size);
                        jobs.push(Job { url: artifact.url.clone(), path: path.clone(), size });
                    }
                    classpath.push(path);
                }
            }
        }
        // 1.13-1.18 ship natives as classifier jars that must be extracted.
        let Some(downloads) = &lib.downloads else { continue };
        let classifier = lib.natives.as_ref().and_then(|n| n.get(os_name())).map(|c| {
            c.replace("${arch}", if cfg!(target_pointer_width = "64") { "64" } else { "32" })
        });
        let native = classifier
            .as_ref()
            .and_then(|c| downloads.classifiers.as_ref()?.get(c))
            .filter(|a| a.path.is_some());
        if let Some(artifact) = native {
            let path = libraries_dir.join(artifact.path.as_ref().unwrap());
            jobs.push(Job { url: artifact.url.clone(), path: path.clone(), size: Some(artifact.size) });
            let exclude = lib.extract.as_ref().map(|e| e.exclude.clone()).unwrap_or_default();
            natives.push((path, exclude));
        }
    }
    classpath.push(client_jar);
    // One pass for libraries and assets keeps every connection busy until the end.
    jobs.extend(asset_jobs(ctx.client, paths, &version.asset_index, progress)?);
    download_all(ctx.client, &jobs, threads, "Downloading game files", progress)?;

    let natives_dir = version_dir.join("natives");
    if !natives.is_empty() {
        progress("Extracting natives...".into(), 1.0);
        for (jar, exclude) in &natives {
            extract_natives(jar, &natives_dir, exclude)?;
        }
    }

    Ok(Prepared { version, classpath, natives_dir, java })
}
