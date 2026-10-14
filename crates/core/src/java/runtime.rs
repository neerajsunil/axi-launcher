//! Mojang's bundled Java runtimes (the same ones the official launcher uses).

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use reqwest::blocking::Client;
use serde_json::Value;

use super::detect::parse_major;
use crate::net::{Job, download_all, get_json};
use crate::{Error, Progress, Result};

const JAVA_RUNTIMES_URL: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

fn platform() -> &'static str {
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

/// Path of the java executable inside a runtime directory.
pub(crate) fn java_in(base: &Path) -> PathBuf {
    if cfg!(windows) {
        base.join("bin").join("javaw.exe")
    } else if cfg!(target_os = "macos") {
        base.join("jre.bundle/Contents/Home/bin/java")
    } else {
        base.join("bin").join("java")
    }
}

/// Mojang's runtime components for this platform with their Java version, newest first,
/// one per major version (`java-runtime-gamma` over `java-runtime-gamma-snapshot`).
fn components(all: &Value) -> Vec<(String, String)> {
    let Some(platform) = all[platform()].as_object() else { return Vec::new() };
    let mut found: Vec<(String, String)> = platform
        .iter()
        .filter_map(|(name, builds)| Some((name.clone(), builds[0]["version"]["name"].as_str()?.to_string())))
        .filter(|(_, version)| parse_major(version) > 0)
        .collect();
    found.sort_by(|a, b| parse_major(&b.1).cmp(&parse_major(&a.1)).then_with(|| a.0.len().cmp(&b.0.len())));
    found.dedup_by_key(|(_, version)| parse_major(version));
    found
}

/// Mojang's Java runtimes for this computer as `(component, version)`, newest first.
pub(crate) fn mojang_runtimes(client: &Client) -> Result<Vec<(String, String)>> {
    Ok(components(&get_json(client, JAVA_RUNTIMES_URL)?))
}

/// The java executable of runtime `component` if it is fully downloaded.
pub(crate) fn installed_runtime(runtimes_dir: &Path, component: &str) -> Option<PathBuf> {
    let base = runtimes_dir.join(component);
    let java = java_in(&base);
    (base.join(".complete").exists() && java.exists()).then_some(java)
}

/// Downloads runtime `component` (e.g. `java-runtime-delta`) if needed and returns its java executable.
pub fn ensure_runtime(
    client: &Client,
    runtimes_dir: &Path,
    component: &str,
    threads: usize,
    progress: Progress,
) -> Result<PathBuf> {
    download_runtime(client, runtimes_dir, platform(), component, threads, progress)
}

/// The Intel build of runtime `component`, run through Rosetta 2 on Apple Silicon. Versions before
/// 1.19 ship LWJGL natives that were never built for arm64, so they need an x86_64 JVM there.
/// Kept apart in `x64/` so it never shadows the native runtime of the same name.
pub fn ensure_intel_mac_runtime(
    client: &Client,
    runtimes_dir: &Path,
    component: &str,
    threads: usize,
    progress: Progress,
) -> Result<PathBuf> {
    const ROSETTA: &str = "/Library/Apple/usr/share/rosetta/rosetta";
    if !Path::new(ROSETTA).exists() {
        return Err(Error::msg(
            "This Minecraft version needs Rosetta 2 on Apple Silicon.              Install it by running `softwareupdate --install-rosetta` in Terminal, then try again.",
        ));
    }
    download_runtime(client, &runtimes_dir.join("x64"), "mac-os", component, threads, progress)
}

fn download_runtime(
    client: &Client,
    runtimes_dir: &Path,
    platform: &str,
    component: &str,
    threads: usize,
    progress: Progress,
) -> Result<PathBuf> {
    if let Some(java) = installed_runtime(runtimes_dir, component) {
        return Ok(java);
    }
    let base = runtimes_dir.join(component);
    let java = java_in(&base);

    progress(format!("Fetching Java runtime {component}..."), 0.0);
    let all: Value = get_json(client, JAVA_RUNTIMES_URL)?;
    let manifest_url = all[platform][component][0]["manifest"]["url"]
        .as_str()
        .ok_or_else(|| Error::msg(format!("no {component} runtime for {platform}")))?;
    let manifest: Value = get_json(client, manifest_url)?;
    let files = manifest["files"]
        .as_object()
        .ok_or_else(|| Error::msg("malformed Java runtime manifest"))?;

    let mut jobs = Vec::new();
    let mut executables = Vec::new();
    for (rel, file) in files {
        let path = base.join(rel);
        match file["type"].as_str() {
            Some("directory") => fs::create_dir_all(&path)?,
            Some("file") => {
                let raw = &file["downloads"]["raw"];
                let url = raw["url"]
                    .as_str()
                    .ok_or_else(|| Error::msg(format!("no download for {rel}")))?;
                jobs.push(Job { url: url.to_string(), path: path.clone(), size: raw["size"].as_u64() });
                if file["executable"].as_bool() == Some(true) {
                    executables.push(path);
                }
            }
            #[cfg(unix)]
            Some("link") => {
                if let (Some(target), Some(parent)) = (file["target"].as_str(), path.parent()) {
                    fs::create_dir_all(parent)?;
                    let _ = std::os::unix::fs::symlink(target, &path);
                }
            }
            _ => {}
        }
    }
    download_all(client, &jobs, threads, &format!("Downloading Java ({component})"), progress)?;

    #[cfg(unix)]
    for path in &executables {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    let _ = executables;

    File::create(base.join(".complete"))?;
    Ok(java)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_component_per_major() {
        let mut all = serde_json::json!({});
        all[platform()] = serde_json::json!({
            "jre-legacy": [{ "version": { "name": "8u51" } }],
            "java-runtime-gamma": [{ "version": { "name": "17.0.8" } }],
            "java-runtime-gamma-snapshot": [{ "version": { "name": "17.0.8" } }],
            "java-runtime-delta": [{ "version": { "name": "21.0.7" } }],
            "minecraft-java-exe": [],
        });
        let names: Vec<_> = components(&all).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["java-runtime-delta", "java-runtime-gamma", "jre-legacy"]);
    }
}
