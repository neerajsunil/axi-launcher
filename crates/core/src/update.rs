//! Self-updates from GitHub releases: find a newer release, download it in the background,
//! and swap it in when the launcher quits so the next start runs the new version.
//!
//! A finished download is recorded in `<dir>/pending.json`; until that file exists nothing
//! is installed, so an interrupted download is simply started over.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::util::write_atomic;
use crate::{Error, Result};

const LATEST_URL: &str = "https://api.github.com/repos/neerajsunil/axi-launcher/releases/latest";

/// End of the release asset name built for this platform (see `.github/workflows/build.yml`).
const ASSET_SUFFIX: Option<&str> = if cfg!(all(windows, target_arch = "x86_64")) {
    Some("-windows-x64.exe")
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some("-macos-arm64.app.tar.gz")
} else {
    None
};

/// Updates can only be installed on platforms that have a release build.
pub const SUPPORTED: bool = ASSET_SUFFIX.is_some();

/// A release newer than the running launcher, with the file for this platform.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub name: String,
    /// Hex SHA-256 of the file, when GitHub lists one.
    pub sha256: Option<String>,
}

/// A downloaded update waiting to be installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub version: String,
    /// File name inside the updates folder.
    pub file: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

/// The latest published release if it is newer than `current` and has a build for this platform.
pub fn check(client: &Client, current: &str) -> Result<Option<Release>> {
    let Some(suffix) = ASSET_SUFFIX else { return Ok(None) };
    let release: GhRelease = client
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .send()?
        .error_for_status()?
        .json()?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    if !is_newer(&version, current) {
        return Ok(None);
    }
    let Some(asset) = release.assets.into_iter().find(|a| a.name.ends_with(suffix)) else { return Ok(None) };
    Ok(Some(Release {
        version,
        url: asset.browser_download_url,
        // The name comes from a web API; keep only its last component.
        name: Path::new(&asset.name).file_name().map_or("update".into(), |n| n.to_string_lossy().into_owned()),
        sha256: asset.digest.and_then(|d| d.strip_prefix("sha256:").map(str::to_string)),
    }))
}

/// `a > b` for versions like `1.2.10`; anything after `-` or `+` is ignored.
pub fn is_newer(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        let core = v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or("");
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    let (mut a, mut b) = (parts(a), parts(b));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

/// The finished download in `dir`, if it is newer than `current`.
pub fn pending(dir: &Path, current: &str) -> Option<Pending> {
    let text = fs::read_to_string(dir.join("pending.json")).ok()?;
    let pending: Pending = serde_json::from_str(&text).ok()?;
    (is_newer(&pending.version, current) && dir.join(&pending.file).is_file()).then_some(pending)
}

/// Downloads `release` into `dir` (replacing older downloads) and marks it ready to install.
pub fn download(client: &Client, release: &Release, dir: &Path) -> Result<Pending> {
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)?;
    let path = dir.join(&release.name);
    let part = dir.join(format!("{}.part", release.name));

    let mut resp = client.get(&release.url).send()?.error_for_status()?;
    let mut file = File::create(&part)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
    }
    file.sync_all()?;
    drop(file);
    if let Some(expected) = &release.sha256 {
        let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if !hash.eq_ignore_ascii_case(expected) {
            let _ = fs::remove_file(&part);
            return Err(Error::msg(format!("{} is corrupted (checksum mismatch)", release.name)));
        }
    }
    fs::rename(&part, &path)?;

    let pending = Pending { version: release.version.clone(), file: release.name.clone() };
    write_atomic(&dir.join("pending.json"), serde_json::to_vec(&pending)?)?;
    Ok(pending)
}

/// Replaces the running launcher with the downloaded update and clears `dir`.
/// The running process keeps working; the new version starts next time.
pub fn install(dir: &Path, pending: &Pending) -> Result<()> {
    let exe = std::env::current_exe()?;
    replace(&exe, &dir.join(&pending.file))?;
    let _ = fs::remove_dir_all(dir);
    Ok(())
}

/// Removes what an earlier install left behind (Windows can't delete a running exe).
pub fn clean_up() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(old_path(&exe));
    }
}

fn old_path(path: &Path) -> PathBuf {
    let mut old = path.as_os_str().to_owned();
    old.push(".old");
    PathBuf::from(old)
}

/// Windows lets a running exe be renamed, so move it aside and put the new one in its place.
#[cfg(windows)]
fn replace(exe: &Path, new: &Path) -> Result<()> {
    let old = old_path(exe);
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old)?;
    // Copy rather than rename: the data folder may be on another drive than the exe.
    if let Err(e) = fs::copy(new, exe) {
        let _ = fs::remove_file(exe);
        let _ = fs::rename(&old, exe);
        return Err(e.into());
    }
    Ok(())
}

/// Swaps the whole `Axi Launcher.app` bundle for the one in the downloaded archive.
#[cfg(target_os = "macos")]
fn replace(exe: &Path, new: &Path) -> Result<()> {
    // <bundle>.app/Contents/MacOS/axi-launcher
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|b| b.extension().is_some_and(|e| e == "app"))
        .ok_or_else(|| Error::msg("the launcher is not running from an .app bundle"))?;
    let parent = bundle.parent().ok_or_else(|| Error::msg("the .app bundle has no parent folder"))?;
    let staging = parent.join(".axi-update");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let unpacked = (|| -> Result<PathBuf> {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(File::open(new)?));
        tar.set_preserve_permissions(true);
        tar.unpack(&staging)?;
        fs::read_dir(&staging)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| Error::msg("the update has no .app bundle"))
    })();
    let result = unpacked.and_then(|app| {
        let old = old_path(bundle);
        if old.exists() {
            fs::remove_dir_all(&old)?;
        }
        fs::rename(bundle, &old)?;
        if let Err(e) = fs::rename(&app, bundle) {
            let _ = fs::rename(&old, bundle);
            return Err(e.into());
        }
        let _ = fs::remove_dir_all(&old);
        Ok(())
    });
    let _ = fs::remove_dir_all(&staging);
    result
}

#[cfg(not(any(windows, target_os = "macos")))]
fn replace(_exe: &Path, _new: &Path) -> Result<()> {
    Err(Error::msg("updates are not supported on this platform"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.10", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn pending_needs_a_newer_finished_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pending { version: "0.2.0".into(), file: "axi.exe".into() };
        fs::write(dir.path().join("pending.json"), serde_json::to_vec(&p).unwrap()).unwrap();
        assert_eq!(pending(dir.path(), "0.1.0"), None);
        fs::write(dir.path().join("axi.exe"), b"new").unwrap();
        assert_eq!(pending(dir.path(), "0.1.0"), Some(p));
        assert_eq!(pending(dir.path(), "0.2.0"), None);
    }
}
