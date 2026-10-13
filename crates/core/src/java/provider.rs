//! Java builds that can be downloaded, from every provider the launcher knows.

use std::fs;
use std::path::{Component, Path, PathBuf};

use reqwest::blocking::Client;

use super::detect::{parse_major, read_release};
use super::{archive, runtime, temurin, zulu};
use crate::{Error, Progress, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaProvider {
    /// The runtimes the official launcher uses.
    Mojang,
    /// Eclipse Temurin from Adoptium.
    Temurin,
    /// Azul Zulu.
    Zulu,
}

impl JavaProvider {
    pub const ALL: [Self; 3] = [Self::Mojang, Self::Temurin, Self::Zulu];

    pub fn name(self) -> &'static str {
        match self {
            Self::Mojang => "Mojang",
            Self::Temurin => "Eclipse Temurin",
            Self::Zulu => "Azul Zulu",
        }
    }
}

/// One downloadable Java version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRelease {
    pub provider: JavaProvider,
    pub major: u32,
    /// Latest version, e.g. `21.0.7`; empty when it could not be looked up.
    pub version: String,
    /// Mojang component name or Zulu package id.
    id: String,
}

impl JavaRelease {
    /// Long-term support release (8, 11, 17, 21, 25, ...).
    pub fn is_lts(&self) -> bool {
        matches!(self.major, 8 | 11) || (self.major >= 17 && (self.major - 17) % 4 == 0)
    }

    /// e.g. `Eclipse Temurin 21`.
    pub fn name(&self) -> String {
        format!("{} {}", self.provider.name(), self.major)
    }
}

/// The Minecraft versions that require Java `major`; empty for versions no game asks for.
pub fn games_for_java(major: u32) -> &'static str {
    match major {
        8 => "Minecraft 1.13 – 1.16.5",
        17 => "Minecraft 1.17 – 1.20.4",
        21 => "Minecraft 1.20.5 – 1.21.x",
        25 => "Minecraft 26.1 and newer",
        _ => "",
    }
}

/// Newest Java version any Minecraft release needs; anything above is ahead of the game.
pub const NEWEST_GAME_JAVA: u32 = 25;

/// Every Java version `provider` offers for this computer, newest first.
pub fn available_releases(client: &Client, provider: JavaProvider) -> Result<Vec<JavaRelease>> {
    let release = |major, version: String, id: String| JavaRelease { provider, major, version, id };
    Ok(match provider {
        JavaProvider::Mojang => runtime::mojang_runtimes(client)?
            .into_iter()
            .map(|(component, version)| release(parse_major(&version), version, component))
            .collect(),
        JavaProvider::Temurin => temurin::releases(client)?
            .into_iter()
            .map(|(major, version)| release(major, version, String::new()))
            .collect(),
        JavaProvider::Zulu => zulu::latest(client)?
            .into_iter()
            .filter_map(|p| {
                let major = *p.java_version.first()?;
                let version = p.java_version.iter().map(u32::to_string).collect::<Vec<_>>().join(".");
                Some(release(major, version, p.package_uuid))
            })
            .collect(),
    })
}

/// The java executable of `release` if this launcher already downloaded it.
pub fn installed_release(runtimes_dir: &Path, release: &JavaRelease) -> Option<PathBuf> {
    match release.provider {
        JavaProvider::Mojang => runtime::installed_runtime(runtimes_dir, &release.id),
        JavaProvider::Temurin => archive::installed(&temurin::dir(runtimes_dir, release.major)),
        JavaProvider::Zulu => archive::installed(&zulu::dir(runtimes_dir, release.major)),
    }
}

/// Version of the Java at `java` from its `release` file, e.g. `21.0.4` or `1.8.0_412`.
pub fn installed_version(java: &Path) -> Option<String> {
    read_release(java).map(|(version, _)| version)
}

/// Comparable version numbers: `1.8.0_412` and `8.0.412` -> [8, 0, 412]; build suffixes are ignored.
fn version_key(version: &str) -> Vec<u32> {
    let version = version.split(['+', '-']).next().unwrap_or_default();
    let mut key: Vec<u32> = version.split(['.', '_']).map_while(|p| p.parse().ok()).collect();
    if key.first() == Some(&1) && key.len() > 1 {
        key.remove(0);
    }
    key
}

/// Whether `installed` is an older build than `latest`; false when either is unknown.
pub fn is_outdated(installed: &str, latest: &str) -> bool {
    let (installed, latest) = (version_key(installed), version_key(latest));
    !installed.is_empty() && !latest.is_empty() && installed < latest
}

/// Deletes the Java whose executable is `java`. Only runtimes this launcher downloaded
/// (inside `runtimes_dir`) can be removed; the whole runtime folder goes.
pub fn remove_install(runtimes_dir: &Path, java: &Path) -> Result<()> {
    let not_managed = || Error::msg("Only Java downloaded by this launcher can be deleted");
    let rel = java.strip_prefix(runtimes_dir).map_err(|_| not_managed())?;
    let Some(Component::Normal(folder)) = rel.components().next() else { return Err(not_managed()) };
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(not_managed());
    }
    let dir = runtimes_dir.join(folder);
    if !dir.is_dir() {
        return Err(not_managed());
    }
    // Drop the completion marker first so a half-deleted folder never counts as installed.
    let _ = fs::remove_file(dir.join(".complete"));
    fs::remove_dir_all(&dir)?;
    Ok(())
}

/// Downloads the latest build of `release` into `runtimes_dir`, replacing an older one; returns its java executable.
pub fn install_release(
    client: &Client,
    runtimes_dir: &Path,
    release: &JavaRelease,
    threads: usize,
    progress: Progress,
) -> Result<PathBuf> {
    match release.provider {
        JavaProvider::Mojang => {
            // Mojang replaces a component's files in place; forget the old build so it downloads again.
            let _ = fs::remove_file(runtimes_dir.join(&release.id).join(".complete"));
            runtime::ensure_runtime(client, runtimes_dir, &release.id, threads, progress)
        }
        JavaProvider::Temurin => temurin::install(client, runtimes_dir, release.major, progress),
        JavaProvider::Zulu => zulu::install(client, runtimes_dir, release.major, &release.id, progress),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lts_versions() {
        let lts = |major| JavaRelease { provider: JavaProvider::Temurin, major, version: String::new(), id: String::new() }.is_lts();
        assert!([8, 11, 17, 21, 25, 29].into_iter().all(lts));
        assert!(![16, 22, 26, 27].into_iter().any(lts));
    }

    #[test]
    fn outdated_builds() {
        assert!(is_outdated("21.0.4", "21.0.8"));
        assert!(is_outdated("1.8.0_412", "8.0.462"));
        assert!(!is_outdated("1.8.0_462", "8.0.462"));
        assert!(!is_outdated("21.0.8", "21.0.8+9"));
        assert!(!is_outdated("21.0.10", "21.0.8"));
        assert!(!is_outdated("", "21.0.8"));
    }

    #[test]
    fn removes_only_managed_installs() {
        let dir = tempfile::tempdir().unwrap();
        let runtimes = dir.path().join("runtimes");
        let java = runtimes.join("temurin-21").join("bin").join("java");
        fs::create_dir_all(java.parent().unwrap()).unwrap();
        fs::write(&java, "").unwrap();

        assert!(remove_install(&runtimes, &dir.path().join("elsewhere/bin/java")).is_err());
        assert!(remove_install(&runtimes, &runtimes.join("../escape/bin/java")).is_err());
        remove_install(&runtimes, &java).unwrap();
        assert!(!runtimes.join("temurin-21").exists());
        assert!(runtimes.exists());
    }
}
