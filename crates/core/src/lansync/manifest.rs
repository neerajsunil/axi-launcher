//! The synced state of an instance: its game setup plus every synced file and its hash.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::identity::hex;
use super::rules;
use crate::Result;
use crate::instance::{Instance, InstanceConfig, PackOrigin};
use crate::loader::ModLoader;
use crate::mods::{ContentKind, ModSource, list_installed};
use crate::util::write_atomic;

/// Stands for the game setup in the path-to-hash maps the merge works on.
pub const META_PATH: &str = "@meta";
/// Launcher-only folder in the instance with hash caches, merge bases and backups.
pub const SYNC_DIR: &str = "sync";
const HASH_CACHE: &str = "hashes.json";

/// The parts of `instance.toml` that are the same on every computer. Play time, Java and
/// memory settings stay per computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedMeta {
    pub name: String,
    pub game_version: String,
    pub loader: ModLoader,
    pub loader_version: String,
    pub group: String,
    pub notes: String,
    pub pack: Option<PackOrigin>,
}

impl SyncedMeta {
    pub fn of(config: &InstanceConfig) -> Self {
        Self {
            name: config.name.clone(),
            game_version: config.game_version.clone(),
            loader: config.loader,
            loader_version: config.loader_version.clone(),
            group: config.group.clone(),
            notes: config.notes.clone(),
            pack: config.pack.clone(),
        }
    }

    pub fn apply(&self, config: &mut InstanceConfig) {
        config.name = self.name.clone();
        config.game_version = self.game_version.clone();
        config.loader = self.loader;
        config.loader_version = self.loader_version.clone();
        config.group = self.group.clone();
        config.notes = self.notes.clone();
        config.pack = self.pack.clone();
    }

    pub fn hash(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        hex(&Sha256::digest(&json))
    }
}

/// Where a content file can be downloaded from instead of copying it over the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub source: ModSource,
    pub kind: ContentKind,
    pub project_id: String,
    pub file_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    /// Unix milliseconds.
    pub modified: u64,
    /// SHA-256, hex.
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub meta: SyncedMeta,
    /// Sorted by path.
    pub files: Vec<FileEntry>,
}

/// A path's version: its hash and when it last changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub hash: String,
    pub modified: u64,
}

impl Manifest {
    /// Every path including [`META_PATH`], with its version.
    pub fn versions(&self) -> BTreeMap<String, Version> {
        let mut map: BTreeMap<String, Version> = self
            .files
            .iter()
            .map(|f| (f.path.clone(), Version { hash: f.hash.clone(), modified: f.modified }))
            .collect();
        map.insert(META_PATH.into(), Version { hash: self.meta.hash(), modified: 0 });
        map
    }

    /// Path to hash, as stored for the next merge.
    pub fn hashes(&self) -> BTreeMap<String, String> {
        self.versions().into_iter().map(|(path, v)| (path, v.hash)).collect()
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.binary_search_by(|f| f.path.as_str().cmp(path)).ok().map(|i| &self.files[i])
    }
}

pub fn sync_dir(instance: &Instance) -> PathBuf {
    instance.dir.join(SYNC_DIR)
}

/// Hashes are cached by size and modification time so unchanged worlds aren't re-read.
type HashCache = HashMap<String, (u64, u64, String)>;

/// Lists and hashes every synced file of `instance`.
pub fn scan(instance: &Instance) -> Result<Manifest> {
    let cache_file = sync_dir(instance).join(HASH_CACHE);
    let old_cache: HashCache =
        fs::read(&cache_file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let origins = content_origins(instance);

    let mut found = Vec::new();
    collect(&instance.dir, &instance.dir, &mut found)?;
    let mut cache = HashCache::new();
    let mut files = Vec::with_capacity(found.len());
    for (path, full) in found {
        let Ok(meta) = fs::metadata(&full) else { continue };
        let modified = meta.modified().ok().map_or(0, millis);
        let hash = match old_cache.get(&path) {
            Some((size, time, hash)) if *size == meta.len() && *time == modified => hash.clone(),
            _ => match hash_file(&full) {
                Ok(hash) => hash,
                // Deleted or locked while scanning; it syncs next time.
                Err(_) => continue,
            },
        };
        cache.insert(path.clone(), (meta.len(), modified, hash.clone()));
        files.push(FileEntry { origin: origins.get(&path).cloned(), path, size: meta.len(), modified, hash });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if cache != old_cache {
        write_atomic(&cache_file, serde_json::to_vec(&cache)?)?;
    }
    Ok(Manifest { meta: SyncedMeta::of(&instance.config), files })
}

fn collect(instance_dir: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries.flatten() {
        let full = entry.path();
        let Some(path) = rules::relative(instance_dir, &full) else { continue };
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            // Only descend where synced files can be.
            let probe = format!("{path}/x");
            if path == ".minecraft" || rules::is_synced(&probe) {
                collect(instance_dir, &full, out)?;
            }
        } else if rules::is_synced(&path) {
            out.push((path, full));
        }
    }
    Ok(())
}

/// Content files installed from Modrinth or CurseForge, by path.
fn content_origins(instance: &Instance) -> HashMap<String, Origin> {
    let mut origins = HashMap::new();
    for kind in ContentKind::INSTANCE {
        for item in list_installed(instance, kind).unwrap_or_default() {
            let Some(entry) = item.entry else { continue };
            let suffix = if item.enabled { "" } else { ".disabled" };
            let path = format!(".minecraft/{}/{}{suffix}", kind.folder(), item.file_name);
            origins.insert(
                path,
                Origin { source: entry.source, kind, project_id: entry.project_id, file_id: entry.file_id },
            );
        }
    }
    origins
}

pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{GameSetup, InstanceManager};

    #[test]
    fn scans_only_synced_files() {
        let tmp = tempfile::tempdir().unwrap();
        let instance = InstanceManager::new(tmp.path()).create("A", "", GameSetup::vanilla("1.21.10")).unwrap();
        let game = instance.game_dir();
        for (path, text) in [
            ("saves/W/level.dat", "level"),
            ("saves/W/session.lock", "lock"),
            ("mods/a.jar", "jar"),
            ("config/a.json", "{}"),
            ("options.txt", "fov:90"),
            ("servers.dat", "servers"),
        ] {
            let full = game.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, text).unwrap();
        }
        let manifest = scan(&instance).unwrap();
        let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, [".minecraft/mods/a.jar", ".minecraft/saves/W/level.dat", ".minecraft/servers.dat"]);
        assert_eq!(manifest.file(".minecraft/mods/a.jar").unwrap().size, 3);
        assert_eq!(manifest.meta.game_version, "1.21.10");
        // Second scan uses the cache and agrees.
        assert_eq!(scan(&instance).unwrap(), manifest);
        assert!(manifest.versions().contains_key(META_PATH));
    }
}
