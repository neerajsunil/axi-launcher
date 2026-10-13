//! The list of all game versions.

use std::path::Path;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::util::write_atomic;

const VERSION_MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Release time of 1.13, the first version using the modern launch format.
const OLDEST_SUPPORTED: &str = "2018-07-18T15:11:46+00:00";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub release_time: String,
}

impl VersionEntry {
    pub fn is_release(&self) -> bool {
        self.kind == "release"
    }

    /// Releases and snapshots from 1.13 onwards.
    pub fn is_supported(&self) -> bool {
        matches!(self.kind.as_str(), "release" | "snapshot")
            && self.release_time.as_str() >= OLDEST_SUPPORTED
    }

    /// `YYYY-MM-DD` part of the release time.
    pub fn release_date(&self) -> &str {
        self.release_time.get(..10).unwrap_or(&self.release_time)
    }
}

#[derive(Serialize, Deserialize)]
struct VersionManifest {
    versions: Vec<VersionEntry>,
}

/// Fetches supported versions (newest first), caching them so the list also works offline.
pub fn fetch_versions(client: &Client, cache_file: &Path) -> Result<Vec<VersionEntry>> {
    let manifest = match crate::net::get_json::<VersionManifest>(client, VERSION_MANIFEST_URL) {
        Ok(manifest) => {
            let _ = write_atomic(cache_file, serde_json::to_vec(&manifest)?);
            manifest
        }
        Err(e) => match std::fs::read(cache_file) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(_) => return Err(e),
        },
    };
    Ok(manifest.versions.into_iter().filter(VersionEntry::is_supported).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, time: &str) -> VersionEntry {
        VersionEntry { id: "x".into(), kind: kind.into(), url: String::new(), release_time: time.into() }
    }

    #[test]
    fn supported_versions() {
        assert!(entry("release", OLDEST_SUPPORTED).is_supported());
        assert!(entry("snapshot", "2025-03-01T10:00:00+00:00").is_supported());
        assert!(!entry("release", "2017-09-18T08:39:46+00:00").is_supported());
        assert!(!entry("old_beta", "2019-01-01T00:00:00+00:00").is_supported());
    }
}
