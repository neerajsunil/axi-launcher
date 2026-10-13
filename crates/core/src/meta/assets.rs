//! Game assets (sounds, languages, ...), stored by hash and shared by all instances.

use std::collections::HashMap;
use std::fs;

use reqwest::blocking::Client;
use serde::Deserialize;

use super::version::AssetIndexRef;
use crate::net::{Job, download};
use crate::paths::Paths;
use crate::{Progress, Result};

const RESOURCES_URL: &str = "https://resources.download.minecraft.net";

#[derive(Deserialize)]
struct AssetIndex {
    objects: HashMap<String, AssetObject>,
}

#[derive(Deserialize)]
struct AssetObject {
    hash: String,
    size: u64,
}

/// Fetches the asset index and returns the objects to download.
pub fn asset_jobs(client: &Client, paths: &Paths, index_ref: &AssetIndexRef, progress: Progress) -> Result<Vec<Job>> {
    progress("Fetching asset index...".into(), 0.0);
    let assets_dir = paths.assets_dir();
    let index_path = assets_dir.join("indexes").join(format!("{}.json", index_ref.id));
    download(client, &index_ref.url, &index_path, Some(index_ref.size))?;
    let index: AssetIndex = serde_json::from_slice(&fs::read(&index_path)?)?;

    Ok(index
        .objects
        .values()
        .map(|o| {
            let prefix = &o.hash[..2];
            Job {
                url: format!("{RESOURCES_URL}/{prefix}/{}", o.hash),
                path: assets_dir.join("objects").join(prefix).join(&o.hash),
                size: Some(o.size),
            }
        })
        .collect())
}
