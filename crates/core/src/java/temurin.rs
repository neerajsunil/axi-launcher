//! Eclipse Temurin JREs from Adoptium.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use reqwest::blocking::Client;
use serde::Deserialize;

use super::archive::{self, Archive};
use crate::net::get_json;
use crate::{Error, Progress, Result};

const API: &str = "https://api.adoptium.net/v3";

#[derive(Deserialize)]
struct Releases {
    available_releases: Vec<u32>,
}

#[derive(Deserialize)]
struct Asset {
    binary: Binary,
    version: Version,
}

#[derive(Deserialize)]
struct Binary {
    package: Package,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    link: String,
    checksum: String,
}

#[derive(Deserialize)]
struct Version {
    semver: String,
}

/// Adoptium's names for this OS and architecture.
fn platform() -> Option<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "mac",
        "linux" => "linux",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "x32",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some((os, arch))
}

/// Where Temurin `major` is installed.
pub(crate) fn dir(runtimes_dir: &Path, major: u32) -> PathBuf {
    runtimes_dir.join(format!("temurin-{major}"))
}

/// The latest JRE build of `major` for this computer; `None` when Adoptium has none.
fn latest(client: &Client, major: u32) -> Result<Option<Asset>> {
    let Some((os, arch)) = platform() else { return Ok(None) };
    let url = format!("{API}/assets/latest/{major}/hotspot?architecture={arch}&image_type=jre&os={os}&vendor=eclipse");
    match get_json::<Vec<Asset>>(client, &url) {
        Ok(assets) => Ok(assets.into_iter().next()),
        // Adoptium answers 404 for versions it does not build.
        Err(Error::Http(e)) if e.status() == Some(reqwest::StatusCode::NOT_FOUND) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `21.0.8+9.0.LTS` -> `21.0.8`.
fn short_version(semver: &str) -> String {
    semver.split(['+', '-']).next().unwrap_or(semver).to_string()
}

/// Every Java version Adoptium has a JRE of for this computer, with its latest version, newest first.
/// The version is empty when it could not be looked up.
pub(crate) fn releases(client: &Client) -> Result<Vec<(u32, String)>> {
    let majors = get_json::<Releases>(client, &format!("{API}/info/available_releases"))?.available_releases;
    let mut found: Vec<(u32, String)> = majors
        .into_par_iter()
        .filter(|&major| major >= 8)
        .filter_map(|major| match latest(client, major) {
            Ok(Some(asset)) => Some((major, short_version(&asset.version.semver))),
            Ok(None) => None,
            Err(_) => Some((major, String::new())),
        })
        .collect();
    found.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    Ok(found)
}

/// Downloads the latest Temurin `major` JRE, replacing any older build, and returns its java executable.
pub(crate) fn install(client: &Client, runtimes_dir: &Path, major: u32, progress: Progress) -> Result<PathBuf> {
    let dest = dir(runtimes_dir, major);
    progress(format!("Looking up Temurin {major}..."), 0.0);
    let asset = latest(client, major)?
        .ok_or_else(|| Error::msg(format!("Temurin has no Java {major} build for this computer")))?;
    let package = &asset.binary.package;
    let archive = Archive { name: &package.name, url: &package.link, sha256: &package.checksum };
    let label = format!("Temurin {}", short_version(&asset.version.semver));
    archive::install(client, runtimes_dir, &archive, &dest, &label, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_versions() {
        assert_eq!(short_version("21.0.8+9.0.LTS"), "21.0.8");
        assert_eq!(short_version("8.0.462+8"), "8.0.462");
        assert_eq!(short_version("25"), "25");
    }
}
