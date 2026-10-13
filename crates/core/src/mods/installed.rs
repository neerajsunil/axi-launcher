//! Content installed in an instance: mods, resource packs and shaders, each in its own
//! `.minecraft` folder. Where each file came from is recorded in `<instance>/content.toml`
//! so it can be updated later. Disabled files are renamed to `<file>.disabled`, which the
//! game ignores.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use serde::{Deserialize, Serialize};

use super::kind::SHADER_LOADERS;
use super::{ContentKind, ModApi, ModFile, ModSource, ProjectInfo};
use crate::instance::Instance;
use crate::net::{download, pool};
use crate::util::write_atomic;
use crate::{Error, Progress, Result};

const INDEX_FILE: &str = "content.toml";
/// Used before resource packs and shaders were supported; only lists mods.
const LEGACY_INDEX_FILE: &str = "mods.toml";
const DISABLED: &str = ".disabled";

/// Where an installed file came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentEntry {
    /// Enabled file name (without `.disabled`).
    pub file_name: String,
    pub title: String,
    pub source: ModSource,
    pub project_id: String,
    pub file_id: String,
    pub version_number: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    /// Installed automatically because another mod requires it.
    #[serde(default)]
    pub dependency: bool,
    /// Chosen by hand instead of the newest version, so update checks leave it alone.
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    mods: Vec<ContentEntry>,
    #[serde(default)]
    resourcepacks: Vec<ContentEntry>,
    #[serde(default)]
    shaderpacks: Vec<ContentEntry>,
}

impl Index {
    fn list(&self, kind: ContentKind) -> &Vec<ContentEntry> {
        match kind {
            ContentKind::ResourcePack => &self.resourcepacks,
            ContentKind::Shader => &self.shaderpacks,
            _ => &self.mods,
        }
    }

    fn entries(&mut self, kind: ContentKind) -> &mut Vec<ContentEntry> {
        match kind {
            ContentKind::ResourcePack => &mut self.resourcepacks,
            ContentKind::Shader => &mut self.shaderpacks,
            _ => &mut self.mods,
        }
    }
}

/// A file in one of the content folders, with its metadata when it was installed through the launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledContent {
    pub file_name: String,
    pub enabled: bool,
    pub title: String,
    pub version: String,
    pub entry: Option<ContentEntry>,
}

fn load_index(instance: &Instance) -> Result<Index> {
    for name in [INDEX_FILE, LEGACY_INDEX_FILE] {
        match fs::read_to_string(instance.dir.join(name)) {
            Ok(text) => return Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Index::default())
}

fn save_index(instance: &Instance, index: &Index) -> Result<()> {
    write_atomic(&instance.dir.join(INDEX_FILE), toml::to_string_pretty(index)?)?;
    let _ = fs::remove_file(instance.dir.join(LEGACY_INDEX_FILE));
    Ok(())
}

/// Rejects names that could point outside the content folder.
fn check_file_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
        return Err(Error::msg(format!("invalid file name '{name}'")));
    }
    Ok(())
}

fn disabled_path(dir: &Path, file_name: &str) -> PathBuf {
    dir.join(format!("{file_name}{DISABLED}"))
}

/// All files of `kind` in the instance, sorted by title.
pub fn list_installed(instance: &Instance, kind: ContentKind) -> Result<Vec<InstalledContent>> {
    let mut index = load_index(instance)?;
    let known = index.entries(kind);
    let entries = match fs::read_dir(instance.content_dir(kind)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut items: Vec<InstalledContent> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (file_name, enabled) = match name.strip_suffix(DISABLED) {
                Some(base) => (base.to_string(), false),
                None => (name, true),
            };
            if !file_name.to_lowercase().ends_with(kind.extension()) {
                return None;
            }
            let entry = known.iter().find(|m| m.file_name == file_name).cloned();
            Some(InstalledContent {
                title: entry.as_ref().map_or_else(
                    || file_name[..file_name.len() - kind.extension().len()].to_string(),
                    |e| e.title.clone(),
                ),
                version: entry.as_ref().map(|e| e.version_number.clone()).unwrap_or_default(),
                file_name,
                enabled,
                entry,
            })
        })
        .collect();
    items.sort_by_key(|m| m.title.to_lowercase());
    Ok(items)
}

pub fn set_enabled(instance: &Instance, kind: ContentKind, file_name: &str, enabled: bool) -> Result<()> {
    check_file_name(file_name)?;
    let dir = instance.content_dir(kind);
    let (on, off) = (dir.join(file_name), disabled_path(&dir, file_name));
    let (from, to) = if enabled { (off, on) } else { (on, off) };
    if from.exists() {
        fs::rename(from, to)?;
    }
    Ok(())
}

/// Deletes the file and forgets its metadata.
pub fn remove(instance: &Instance, kind: ContentKind, file_name: &str) -> Result<()> {
    check_file_name(file_name)?;
    delete_files(&instance.content_dir(kind), file_name)?;
    let mut index = load_index(instance)?;
    index.entries(kind).retain(|m| m.file_name != file_name);
    save_index(instance, &index)
}

fn delete_files(dir: &Path, file_name: &str) -> Result<()> {
    for path in [dir.join(file_name), disabled_path(dir, file_name)] {
        match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

/// Remembers where files that are already in place came from (modpack installs).
pub fn record(instance: &Instance, kind: ContentKind, entries: Vec<ContentEntry>) -> Result<()> {
    let mut index = load_index(instance)?;
    let list = index.entries(kind);
    for entry in entries {
        list.retain(|m| m.file_name != entry.file_name);
        list.push(entry);
    }
    save_index(instance, &index)
}

/// Whether a mod that can load shader packs (Iris or Oculus) is installed.
pub fn has_shader_loader(instance: &Instance) -> bool {
    let Ok(mut index) = load_index(instance) else { return false };
    let known = index.entries(ContentKind::Mod).iter().any(|m| SHADER_LOADERS.contains(&m.project_id.as_str()));
    known
        || list_installed(instance, ContentKind::Mod).is_ok_and(|mods| {
            mods.iter().filter(|m| m.enabled).any(|m| {
                let name = m.file_name.to_lowercase();
                name.starts_with("iris") || name.starts_with("oculus")
            })
        })
}

/// One project to install: an exact file, or `None` for the newest compatible one.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub source: ModSource,
    pub project_id: String,
    pub file_id: Option<String>,
}

impl InstallRequest {
    pub fn newest(source: ModSource, project_id: impl Into<String>) -> Self {
        Self { source, project_id: project_id.into(), file_id: None }
    }
}

/// What [`install`] did: titles installed, and errors for the rest.
#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<String>,
    pub failed: Vec<String>,
}

/// A file chosen for download.
struct Planned {
    kind: ContentKind,
    info: ProjectInfo,
    file: ModFile,
    url: String,
    is_root: bool,
    pinned: bool,
}

/// Installs (or updates) projects: the newest compatible file, or `file_id` when given
/// (which pins it unless it is the newest). For mods, required dependencies that aren't
/// installed yet come along. Lookups and downloads run `threads` at a time; a request that
/// fails doesn't stop the others.
pub fn install(
    api: &ModApi,
    instance: &Instance,
    kind: ContentKind,
    requests: &[InstallRequest],
    threads: usize,
    progress: Progress,
) -> Result<InstallReport> {
    let config = &instance.config;
    if kind == ContentKind::Modpack {
        return Err(Error::msg("Modpacks are installed as new instances"));
    }
    if kind.needs_loader() && !config.loader.is_modded() {
        return Err(Error::msg(match kind {
            ContentKind::Shader => "Shaders need Iris, which runs on Fabric or NeoForge. Choose a mod loader first.",
            _ => "Choose a mod loader for this instance first",
        }));
    }
    let mut index = load_index(instance)?;
    let pool = pool(threads)?;
    let mut report = InstallReport::default();

    progress("Finding compatible versions...".into(), 0.0);
    let plans: Vec<Result<Vec<Planned>>> =
        pool.install(|| requests.par_iter().map(|r| plan(api, instance, &index, kind, r)).collect());
    // One file per project; a requested project wins over the same one pulled in as a dependency.
    let mut planned: Vec<Planned> = Vec::new();
    for plan in plans {
        match plan {
            Ok(items) => {
                for item in items {
                    let same = |p: &&mut Planned| {
                        p.file.source == item.file.source && p.file.project_id == item.file.project_id
                    };
                    match planned.iter_mut().find(same) {
                        Some(existing) if item.is_root && !existing.is_root => *existing = item,
                        Some(_) => {}
                        None => planned.push(item),
                    }
                }
            }
            Err(e) => report.failed.push(e.to_string()),
        }
    }

    let total = planned.len();
    let done = AtomicUsize::new(0);
    let downloads: Vec<Result<()>> = pool.install(|| {
        planned
            .par_iter()
            .map(|p| {
                let dir = instance.content_dir(p.kind);
                delete_files(&dir, &p.file.file_name)?;
                download(api.client, &p.url, &dir.join(&p.file.file_name), None)?;
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                progress(format!("Downloading ({n}/{total})..."), n as f32 / total as f32);
                Ok(())
            })
            .collect()
    });

    for (p, downloaded) in planned.into_iter().zip(downloads) {
        if let Err(e) = downloaded {
            report.failed.push(format!("{}: {e}", p.info.title));
            continue;
        }
        let entries = index.entries(p.kind);
        // Replace any other version of the same project.
        if let Some(pos) = entries.iter().position(|m| m.source == p.file.source && m.project_id == p.file.project_id) {
            let old = entries.remove(pos);
            if old.file_name != p.file.file_name {
                if let Err(e) = delete_files(&instance.content_dir(p.kind), &old.file_name) {
                    report.failed.push(format!("Could not remove {}: {e}", old.file_name));
                }
            }
        }
        entries.push(ContentEntry {
            file_name: p.file.file_name,
            title: p.info.title.clone(),
            source: p.file.source,
            project_id: p.file.project_id,
            file_id: p.file.file_id,
            version_number: p.file.version_number,
            icon_url: p.info.icon_url,
            dependency: !p.is_root,
            pinned: p.pinned,
        });
        report.installed.push(p.info.title);
    }
    save_index(instance, &index)?;
    progress("Done".into(), 1.0);
    Ok(report)
}

/// Picks the file for `request` and its missing dependencies, one dependency level at a time.
fn plan(
    api: &ModApi,
    instance: &Instance,
    index: &Index,
    kind: ContentKind,
    request: &InstallRequest,
) -> Result<Vec<Planned>> {
    let root = request.project_id.as_str();
    let mut seen = HashSet::new();
    let mut planned = Vec::new();
    let mut level = vec![(kind, root.to_string())];
    while !level.is_empty() {
        level.retain(|(kind, id)| {
            seen.insert(id.clone())
                && (id == root || !index.list(*kind).iter().any(|m| m.source == request.source && m.project_id == *id))
        });
        let resolved = level
            .par_iter()
            .map(|(kind, id)| {
                let is_root = id == root;
                let file_id = request.file_id.as_deref().filter(|_| is_root);
                resolve(api, instance, *kind, request.source, id, file_id).map(|p| Planned { is_root, ..p })
            })
            .collect::<Result<Vec<_>>>()?;
        // Dependencies are always mods (a shader's dependency would be Iris).
        level = resolved.iter().flat_map(|p| p.file.dependencies.iter().map(|d| (ContentKind::Mod, d.clone()))).collect();
        planned.extend(resolved);
    }
    Ok(planned)
}

/// The file to install for one project: `file_id`, or the newest compatible one.
fn resolve(
    api: &ModApi,
    instance: &Instance,
    kind: ContentKind,
    source: ModSource,
    id: &str,
    file_id: Option<&str>,
) -> Result<Planned> {
    let config = &instance.config;
    let info = api.project(source, id)?;
    let latest = api.latest_file(source, kind, id, &config.game_version, config.loader)?;
    let chosen = match file_id {
        Some(file_id) if latest.as_ref().is_none_or(|f| f.file_id != file_id) => {
            api.file(source, kind, id, file_id)?.map(|f| (f, true))
        }
        _ => latest.map(|f| (f, false)),
    };
    let (file, pinned) = chosen.ok_or_else(|| {
        let target = if kind.needs_loader() {
            format!("{} {}", config.loader.display_name(), config.game_version)
        } else {
            format!("Minecraft {}", config.game_version)
        };
        Error::msg(format!("{} has no version for {target}", info.title))
    })?;
    check_file_name(&file.file_name)?;
    let url = file.url.clone().ok_or_else(|| {
        Error::msg(format!(
            "The author of {} only allows downloads from the CurseForge website. \
             Download it there and put it in the {} folder.",
            info.title,
            kind.folder()
        ))
    })?;
    Ok(Planned { kind, info, file, url, is_root: false, pinned })
}

/// File names of installed `kind` content that has a newer compatible version, checked
/// `threads` at a time.
pub fn check_updates(api: &ModApi, instance: &Instance, kind: ContentKind, threads: usize) -> Result<Vec<String>> {
    let config = &instance.config;
    let index = load_index(instance)?;
    let entries = index.list(kind);
    Ok(pool(threads)?.install(|| {
        entries
            .par_iter()
            .filter(|m| {
                !m.pinned
                    && matches!(
                        api.latest_file(m.source, kind, &m.project_id, &config.game_version, config.loader),
                        Ok(Some(f)) if f.file_id != m.file_id
                    )
            })
            .map(|m| m.file_name.clone())
            .collect()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{GameSetup, InstanceManager};

    fn entry(file_name: &str, title: &str, project_id: &str) -> ContentEntry {
        ContentEntry {
            file_name: file_name.into(),
            title: title.into(),
            source: ModSource::Modrinth,
            project_id: project_id.into(),
            file_id: "abc".into(),
            version_number: "0.7".into(),
            icon_url: None,
            dependency: false,
            pinned: false,
        }
    }

    #[test]
    fn list_toggle_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let m = InstanceManager::new(tmp.path());
        let instance = m.create("Modded", "", GameSetup::vanilla("1.21.10")).unwrap();
        assert!(list_installed(&instance, ContentKind::Mod).unwrap().is_empty());

        let dir = instance.mods_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("sodium-0.7.jar"), "x").unwrap();
        fs::write(dir.join("manual.jar"), "x").unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        record(&instance, ContentKind::Mod, vec![entry("sodium-0.7.jar", "Sodium", "AANobbMI")]).unwrap();

        let list = list_installed(&instance, ContentKind::Mod).unwrap();
        assert_eq!(list.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["manual", "Sodium"]);
        assert_eq!(list[1].version, "0.7");

        set_enabled(&instance, ContentKind::Mod, "sodium-0.7.jar", false).unwrap();
        assert!(dir.join("sodium-0.7.jar.disabled").exists());
        let sodium = list_installed(&instance, ContentKind::Mod).unwrap().into_iter().find(|m| m.title == "Sodium");
        assert!(!sodium.unwrap().enabled);
        set_enabled(&instance, ContentKind::Mod, "sodium-0.7.jar", true).unwrap();
        assert!(dir.join("sodium-0.7.jar").exists());

        remove(&instance, ContentKind::Mod, "sodium-0.7.jar").unwrap();
        assert!(!dir.join("sodium-0.7.jar").exists());
        assert!(load_index(&instance).unwrap().mods.is_empty());
        assert!(remove(&instance, ContentKind::Mod, "../instance.toml").is_err());
    }

    #[test]
    fn kinds_use_their_own_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let m = InstanceManager::new(tmp.path());
        let instance = m.create("Packs", "", GameSetup::vanilla("1.21.10")).unwrap();
        let packs = instance.content_dir(ContentKind::ResourcePack);
        fs::create_dir_all(&packs).unwrap();
        fs::write(packs.join("Faithful.zip"), "x").unwrap();
        fs::write(packs.join("mod.jar"), "x").unwrap();
        record(&instance, ContentKind::ResourcePack, vec![entry("Faithful.zip", "Faithful 32x", "faith")]).unwrap();

        let list = list_installed(&instance, ContentKind::ResourcePack).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Faithful 32x");
        assert!(list_installed(&instance, ContentKind::Mod).unwrap().is_empty());
        assert!(!has_shader_loader(&instance));

        record(&instance, ContentKind::Mod, vec![entry("iris.jar", "Iris", "YL57xq9U")]).unwrap();
        assert!(has_shader_loader(&instance));
    }

    #[test]
    fn reads_the_old_mods_index() {
        let tmp = tempfile::tempdir().unwrap();
        let m = InstanceManager::new(tmp.path());
        let instance = m.create("Old", "", GameSetup::vanilla("1.21.10")).unwrap();
        let old = "[[mods]]\nfile_name = \"a.jar\"\ntitle = \"A\"\nsource = \"modrinth\"\nproject_id = \"p\"\nfile_id = \"f\"\nversion_number = \"1\"\n";
        fs::write(instance.dir.join(LEGACY_INDEX_FILE), old).unwrap();
        assert_eq!(load_index(&instance).unwrap().mods[0].title, "A");

        record(&instance, ContentKind::Shader, vec![entry("bsl.zip", "BSL", "s")]).unwrap();
        assert!(!instance.dir.join(LEGACY_INDEX_FILE).exists());
        let mut index = load_index(&instance).unwrap();
        assert_eq!(index.entries(ContentKind::Mod).len(), 1);
        assert_eq!(index.entries(ContentKind::Shader).len(), 1);
    }
}
