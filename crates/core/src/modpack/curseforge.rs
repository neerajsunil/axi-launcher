//! CurseForge modpack zips: a `manifest.json` listing project files plus an overrides folder.

use std::collections::HashMap;

use serde::Deserialize;

use super::{SkippedFile, safe_join};
use crate::instance::{GameSetup, Instance};
use crate::loader::ModLoader;
use crate::mods::{ContentEntry, ContentKind, ModApi, ModSource, curseforge, record};
use crate::net::{Job, download_all};
use crate::{Error, Progress, Result};

pub const MANIFEST: &str = "manifest.json";

#[derive(Debug, Deserialize)]
pub struct Manifest {
    minecraft: Minecraft,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    files: Vec<ManifestFile>,
    #[serde(default = "default_overrides")]
    pub overrides: String,
}

fn default_overrides() -> String {
    "overrides".into()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Minecraft {
    version: String,
    #[serde(default)]
    mod_loaders: Vec<Loader>,
}

#[derive(Debug, Deserialize)]
struct Loader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    #[serde(default = "yes")]
    required: bool,
}

fn yes() -> bool {
    true
}

/// The game version and loader the pack needs.
pub fn game(manifest: &Manifest) -> Result<GameSetup> {
    let loaders = &manifest.minecraft.mod_loaders;
    let loader = loaders.iter().find(|l| l.primary).or(loaders.first());
    let (loader, loader_version) = match loader.map(|l| l.id.as_str()) {
        None => (ModLoader::Vanilla, String::new()),
        Some(id) => {
            if let Some(v) = id.strip_prefix("fabric-") {
                (ModLoader::Fabric, v.to_string())
            } else if let Some(v) = id.strip_prefix("neoforge-") {
                (ModLoader::NeoForge, v.to_string())
            } else if id.starts_with("forge-") {
                return Err(Error::msg("This pack uses Forge, which isn't supported yet. Fabric and NeoForge packs work."));
            } else {
                return Err(Error::msg(format!("This pack uses an unsupported mod loader ({id})")));
            }
        }
    };
    Ok(GameSetup { version: manifest.minecraft.version.clone(), loader, loader_version })
}

pub fn install(
    api: &ModApi,
    manifest: &Manifest,
    instance: &Instance,
    threads: usize,
    progress: Progress,
) -> Result<Vec<SkippedFile>> {
    let key = api.key()?;
    progress("Looking up the pack's files...".into(), 0.05);
    let wanted: Vec<(u64, u64)> =
        manifest.files.iter().filter(|f| f.required).map(|f| (f.project_id, f.file_id)).collect();
    let files = curseforge::pack_files(api.client, key, &wanted)?;

    let mut jobs = Vec::new();
    let mut skipped = Vec::new();
    let mut by_kind: HashMap<ContentKind, Vec<ContentEntry>> = HashMap::new();
    for file in files {
        let (Some(kind), Some(url)) = (file.kind.filter(|k| *k != ContentKind::Modpack), file.url) else {
            skipped.push(SkippedFile { title: file.title, page_url: file.page_url });
            continue;
        };
        let path = safe_join(&instance.content_dir(kind), &file.file_name)?;
        jobs.push(Job { url, path, size: file.size });
        by_kind.entry(kind).or_default().push(ContentEntry {
            file_name: file.file_name,
            title: file.title,
            source: ModSource::CurseForge,
            project_id: file.project_id,
            file_id: file.file_id,
            version_number: file.version_number,
            icon_url: file.icon_url,
            dependency: false,
            pinned: false,
        });
    }
    let report = |text: String, p: f32| progress(text, 0.1 + p * 0.85);
    download_all(api.client, &jobs, threads, "Downloading files", &report)?;
    for (kind, entries) in by_kind {
        record(instance, kind, entries)?;
    }
    Ok(skipped)
}
