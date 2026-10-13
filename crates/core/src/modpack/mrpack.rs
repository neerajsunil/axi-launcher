//! Modrinth's `.mrpack` format (https://support.modrinth.com/en/articles/8802351).

use std::collections::{HashMap, HashSet};

use reqwest::blocking::Client;
use serde::Deserialize;

use super::safe_join;
use crate::instance::{GameSetup, Instance};
use crate::loader::ModLoader;
use crate::mods::{ContentEntry, ContentKind, ModSource, modrinth, record};
use crate::net::{Job, download_all};
use crate::{Error, Progress, Result};

pub const INDEX: &str = "modrinth.index.json";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version_id: String,
    #[serde(default)]
    files: Vec<IndexFile>,
    dependencies: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexFile {
    path: String,
    #[serde(default)]
    hashes: HashMap<String, String>,
    env: Option<Env>,
    downloads: Vec<String>,
    file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Env {
    client: String,
}

/// The game version and loader the pack needs.
pub fn game(index: &Index) -> Result<GameSetup> {
    let deps = &index.dependencies;
    let version = deps.get("minecraft").ok_or_else(|| Error::msg("The pack doesn't say which Minecraft it needs"))?;
    let (loader, loader_version) = if let Some(v) = deps.get("fabric-loader") {
        (ModLoader::Fabric, v.clone())
    } else if let Some(v) = deps.get("neoforge") {
        (ModLoader::NeoForge, v.clone())
    } else if deps.contains_key("forge") {
        return Err(Error::msg("This pack uses Forge, which isn't supported yet. Fabric and NeoForge packs work."));
    } else if deps.contains_key("quilt-loader") {
        return Err(Error::msg("This pack uses Quilt, which isn't supported yet. Fabric and NeoForge packs work."));
    } else {
        (ModLoader::Vanilla, String::new())
    };
    Ok(GameSetup { version: version.clone(), loader, loader_version })
}

pub fn install(client: &Client, index: &Index, instance: &Instance, threads: usize, progress: Progress) -> Result<()> {
    let game_dir = instance.game_dir();
    let files: Vec<&IndexFile> =
        index.files.iter().filter(|f| f.env.as_ref().is_none_or(|e| e.client != "unsupported")).collect();
    let mut jobs = Vec::new();
    for file in &files {
        let url = file.downloads.first().ok_or_else(|| Error::msg(format!("{} has no download link", file.path)))?;
        jobs.push(Job { url: url.clone(), path: safe_join(&game_dir, &file.path)?, size: file.file_size });
    }
    let report = |text: String, p: f32| progress(text, 0.1 + p * 0.85);
    download_all(client, &jobs, threads, "Downloading files", &report)?;

    // Best effort: name the files and make them updatable like ones installed by hand.
    progress("Reading mod details...".into(), 0.96);
    if let Err(e) = record_content(client, &files, instance) {
        eprintln!("could not look up modpack files on Modrinth: {e}");
    }
    Ok(())
}

fn record_content(client: &Client, files: &[&IndexFile], instance: &Instance) -> Result<()> {
    let hashes: Vec<String> = files.iter().filter_map(|f| f.hashes.get("sha1").cloned()).collect();
    let found = modrinth::versions_by_sha1(client, &hashes)?;
    let ids: Vec<String> = found.values().map(|v| v.project_id.clone()).collect::<HashSet<_>>().into_iter().collect();
    let projects = modrinth::projects(client, &ids)?;

    let mut by_kind: HashMap<ContentKind, Vec<ContentEntry>> = HashMap::new();
    for file in files {
        let Some(kind) = ContentKind::from_path(&file.path) else { continue };
        let Some(file_name) = file.path.rsplit('/').next().filter(|_| file.path.matches('/').count() == 1) else {
            continue; // nested deeper than the content folder
        };
        let Some(version) = file.hashes.get("sha1").and_then(|h| found.get(h)) else { continue };
        let Some(project) = projects.get(&version.project_id) else { continue };
        by_kind.entry(kind).or_default().push(ContentEntry {
            file_name: file_name.to_string(),
            title: project.title.clone(),
            source: ModSource::Modrinth,
            project_id: version.project_id.clone(),
            file_id: version.version_id.clone(),
            version_number: version.version_number.clone(),
            icon_url: project.icon_url.clone(),
            dependency: false,
            pinned: false,
        });
    }
    for (kind, entries) in by_kind {
        record(instance, kind, entries)?;
    }
    Ok(())
}
