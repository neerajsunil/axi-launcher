//! Azul Zulu JREs.

use std::path::{Path, PathBuf};

use reqwest::blocking::Client;
use serde::Deserialize;

use super::archive::{self, Archive};
use crate::net::get_json;
use crate::{Progress, Result};

const API: &str = "https://api.azul.com/metadata/v1/zulu/packages";

#[derive(Deserialize)]
pub(crate) struct Package {
    pub package_uuid: String,
    pub java_version: Vec<u32>,
}

#[derive(Deserialize)]
struct Details {
    name: String,
    download_url: String,
    sha256_hash: String,
}

/// Azul's names for this OS and architecture, and the archive type to ask for.
fn platform() -> Option<(&'static str, &'static str, &'static str)> {
    let (os, archive) = match std::env::consts::OS {
        "windows" => ("windows", "zip"),
        "macos" => ("macos", "tar.gz"),
        "linux" => ("linux", "tar.gz"),
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some((os, arch, archive))
}

/// Where Zulu `major` is installed.
pub(crate) fn dir(runtimes_dir: &Path, major: u32) -> PathBuf {
    runtimes_dir.join(format!("zulu-{major}"))
}

/// The newest JRE package of every Java version, newest first.
pub(crate) fn latest(client: &Client) -> Result<Vec<Package>> {
    let Some((os, arch, archive)) = platform() else { return Ok(Vec::new()) };
    let url = format!(
        "{API}/?os={os}&arch={arch}&archive_type={archive}&java_package_type=jre&javafx_bundled=false\
         &latest=true&release_status=ga&availability_types=CA&page_size=200"
    );
    let mut packages: Vec<Package> = get_json(client, &url)?;
    packages.sort_by(|a, b| b.java_version.cmp(&a.java_version));
    packages.dedup_by_key(|p| p.java_version.first().copied());
    Ok(packages)
}

/// Downloads package `uuid` (Java `major`), replacing any older Zulu `major`, and returns its java executable.
pub(crate) fn install(client: &Client, runtimes_dir: &Path, major: u32, uuid: &str, progress: Progress) -> Result<PathBuf> {
    let dest = dir(runtimes_dir, major);
    progress(format!("Looking up Zulu {major}..."), 0.0);
    let details: Details = get_json(client, &format!("{API}/{uuid}"))?;
    let archive = Archive { name: &details.name, url: &details.download_url, sha256: &details.sha256_hash };
    archive::install(client, runtimes_dir, &archive, &dest, &format!("Zulu {major}"), progress)
}
