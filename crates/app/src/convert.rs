//! Conversions between core types and the structs the UI uses.

use std::collections::HashSet;

use axi_core::account::StoredAccount;
use axi_core::instance::{EffectiveSettings, Instance, InstanceConfig, MemoryOverride};
use axi_core::loader::ModLoader;
use axi_core::mods::description::Block;
use axi_core::mods::{ContentKind, InstalledContent, ModHit, ModSource, ProjectDetails, ProjectVersion};
use axi_core::meta::VersionEntry;
use axi_core::paths::Paths;
use axi_core::settings::{GlobalSettings, Theme, WindowSettings};

use crate::format;
use crate::state::Session;
use crate::lan::{Available, SyncState};
use crate::{AccountItem, DescriptionBlock, GlobalSettingsData, ProjectDetailsData, InstanceItem, InstanceSettingsData, LanInstanceItem, ModHitItem, ModItem, ModVersionItem, VersionItem};

pub fn first_letter(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map_or("?".into(), |c| c.to_uppercase().collect())
}

pub fn account_item(account: &StoredAccount, active: bool, busy: bool, head: Option<slint::Image>) -> AccountItem {
    AccountItem {
        id: account.uuid.as_str().into(),
        name: account.username.as_str().into(),
        initial: first_letter(&account.username).into(),
        has_head: head.is_some(),
        head: head.unwrap_or_default(),
        microsoft: account.is_microsoft(),
        active,
        busy,
    }
}

pub fn instance_item(
    instance: &Instance,
    session: Option<&Session>,
    icon: Option<slint::Image>,
    (sync_state, sync_status): (SyncState, String),
) -> InstanceItem {
    let c = &instance.config;
    let task = session.and_then(|s| s.task.clone());
    InstanceItem {
        id: instance.id.as_str().into(),
        name: c.name.as_str().into(),
        initial: first_letter(&c.name).into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        version: c.game_version.as_str().into(),
        loader: c.loader.display_name().into(),
        loader_index: loader_index(c.loader),
        loader_version: c.loader_version.as_str().into(),
        group: c.group.as_str().into(),
        last_played: format::time_ago(c.last_played).into(),
        playtime: format::duration(c.total_playtime_secs).into(),
        created: format::date(c.created).into(),
        notes: c.notes.as_str().into(),
        pack: c.pack.as_ref().map(|p| format!("{} {}", p.name, p.version).trim().to_string()).unwrap_or_default().into(),
        running: session.is_some_and(|s| s.game.is_some()),
        busy: task.is_some(),
        status: task.as_ref().map(|t| t.0.as_str()).unwrap_or_default().into(),
        progress: task.map_or(0.0, |t| t.1),
        lan_sync: instance.synced().is_some(),
        sync_status: sync_status.into(),
        sync_state: sync_state as i32,
    }
}

pub fn lan_instance_item(available: Available) -> LanInstanceItem {
    let a = &available.advert;
    LanInstanceItem {
        sync_id: a.sync_id.as_str().into(),
        name: a.name.as_str().into(),
        detail: format!("{} · {}", a.game_version, a.loader.display_name()).into(),
        device: available.device.into(),
    }
}

/// Position in the UI's loader list (Vanilla, Fabric, NeoForge).
pub fn loader_index(loader: ModLoader) -> i32 {
    ModLoader::ALL.iter().position(|l| *l == loader).unwrap_or(0) as i32
}

pub fn loader_from_index(index: i32) -> ModLoader {
    ModLoader::ALL.get(index as usize).copied().unwrap_or_default()
}

/// 0 = mods, 1 = resource packs, 2 = shaders, 3 = modpacks.
pub fn kind_from_index(index: i32) -> ContentKind {
    match index {
        1 => ContentKind::ResourcePack,
        2 => ContentKind::Shader,
        3 => ContentKind::Modpack,
        _ => ContentKind::Mod,
    }
}

pub fn kind_index(kind: ContentKind) -> i32 {
    match kind {
        ContentKind::Mod => 0,
        ContentKind::ResourcePack => 1,
        ContentKind::Shader => 2,
        ContentKind::Modpack => 3,
    }
}

/// 0 = Modrinth, 1 = CurseForge.
pub fn source_from_index(index: i32) -> ModSource {
    if index == 1 { ModSource::CurseForge } else { ModSource::Modrinth }
}

pub fn mod_item(m: &InstalledContent, updates: &HashSet<String>, installing: bool, icon: Option<slint::Image>) -> ModItem {
    ModItem {
        file_name: m.file_name.as_str().into(),
        title: m.title.as_str().into(),
        version: m.version.as_str().into(),
        enabled: m.enabled,
        source: m.entry.as_ref().map_or("", |e| e.source.display_name()).into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        initial: first_letter(&m.title).into(),
        dependency: m.entry.as_ref().is_some_and(|e| e.dependency),
        update_available: updates.contains(&m.file_name),
        has_versions: m.entry.is_some(),
        pinned: m.entry.as_ref().is_some_and(|e| e.pinned),
        installing,
    }
}

/// A row in the version picker.
pub fn version_choice(v: &ProjectVersion, current: bool) -> ModVersionItem {
    // Long lists of game versions are cut down to the newest few.
    let shown: Vec<&str> = v.game_versions.iter().rev().take(3).map(String::as_str).collect();
    let mut games = shown.join(", ");
    if v.game_versions.len() > shown.len() {
        games.push_str(&format!(" +{}", v.game_versions.len() - shown.len()));
    }
    let loaders: Vec<String> = v.loaders.iter().map(|l| capitalize(l)).collect();
    let detail = [games, loaders.join(", ")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    ModVersionItem {
        file_id: v.file_id.as_str().into(),
        version: v.version_number.as_str().into(),
        detail: detail.into(),
        date: v.date.as_str().into(),
        stable: v.stable,
        compatible: v.compatible,
        current,
    }
}

fn capitalize(word: &str) -> String {
    match word {
        "neoforge" => "NeoForge".into(),
        _ => {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
        }
    }
}

pub fn hit_item(h: &ModHit, installed: bool, installing: bool, icon: Option<slint::Image>) -> ModHitItem {
    ModHitItem {
        project_id: h.project_id.as_str().into(),
        title: h.title.as_str().into(),
        author: h.author.as_str().into(),
        description: h.description.as_str().into(),
        downloads: format::count(h.downloads).into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        initial: first_letter(&h.title).into(),
        page_url: h.page_url.as_str().into(),
        installed,
        installing,
    }
}

pub fn project_details(d: &ProjectDetails, author: &str, page_url: &str, icon: Option<slint::Image>) -> ProjectDetailsData {
    let environment = match (d.client_side.as_str(), d.server_side.as_str()) {
        ("required" | "optional", "required" | "optional") => "Client and server",
        ("required" | "optional", _) => "Client",
        (_, "required" | "optional") => "Server",
        _ => "",
    };
    ProjectDetailsData {
        title: d.title.as_str().into(),
        summary: d.summary.as_str().into(),
        author: author.into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        initial: first_letter(&d.title).into(),
        downloads: format::count(d.downloads).into(),
        followers: format::count(d.followers).into(),
        updated: d.updated.as_str().into(),
        published: d.published.as_str().into(),
        license: d.license.as_str().into(),
        environment: environment.into(),
        categories: d.categories.iter().map(|c| capitalize(&c.replace('-', " "))).collect::<Vec<_>>().join(", ").into(),
        page_url: page_url.into(),
    }
}

pub fn description_block(block: &Block, image: Option<slint::Image>) -> DescriptionBlock {
    let (kind, text) = match block {
        Block::Heading(t) => (0, t.as_str()),
        Block::Paragraph(t) => (1, t.as_str()),
        Block::Bullet(t) => (2, t.as_str()),
        Block::Image(_) => (3, ""),
        Block::Code(t) => (4, t.as_str()),
        Block::Rule => (5, ""),
    };
    DescriptionBlock { kind, text: text.into(), has_image: image.is_some(), image: image.unwrap_or_default() }
}

pub fn version_item(v: &VersionEntry) -> VersionItem {
    VersionItem { id: v.id.as_str().into(), kind: v.kind.as_str().into(), date: v.release_date().into() }
}

/// Instance settings for the form; fields that aren't overridden show the global value.
pub fn instance_settings(instance: &Instance, global: &GlobalSettings) -> InstanceSettingsData {
    let c = &instance.config;
    let o = &c.overrides;
    let e = EffectiveSettings::resolve(global, c);
    InstanceSettingsData {
        name: c.name.as_str().into(),
        group: c.group.as_str().into(),
        notes: c.notes.as_str().into(),
        java_override: o.java_path.is_some(),
        java_path: e.java_path.as_str().into(),
        memory_override: o.memory.is_some(),
        min_memory: e.min_memory_mb.to_string().into(),
        max_memory: e.max_memory_mb.to_string().into(),
        jvm_override: o.jvm_args.is_some(),
        jvm_args: e.jvm_args.join(" ").into(),
        window_override: o.window.is_some(),
        width: e.window.width.to_string().into(),
        height: e.window.height.to_string().into(),
        fullscreen: e.window.fullscreen,
    }
}

pub fn apply_instance_settings(data: &InstanceSettingsData, config: &mut InstanceConfig) -> Result<(), String> {
    let name = data.name.trim();
    if name.is_empty() {
        return Err("Name cannot be empty".into());
    }
    let memory = if data.memory_override {
        let (min_mb, max_mb) = parse_memory(&data.min_memory, &data.max_memory)?;
        Some(MemoryOverride { min_mb, max_mb })
    } else {
        None
    };
    let window = if data.window_override { Some(parse_window(&data.width, &data.height, data.fullscreen)?) } else { None };

    config.name = name.to_string();
    config.group = data.group.trim().to_string();
    config.notes = data.notes.to_string();
    let o = &mut config.overrides;
    o.java_path = data.java_override.then(|| data.java_path.trim().to_string());
    o.memory = memory;
    o.jvm_args = data.jvm_override.then(|| data.jvm_args.trim().to_string());
    o.window = window;
    Ok(())
}

pub fn global_settings(s: &GlobalSettings, paths: &Paths) -> GlobalSettingsData {
    GlobalSettingsData {
        theme: match s.theme {
            Theme::System => 0,
            Theme::Light => 1,
            Theme::Dark => 2,
        },
        java_path: s.java.path.as_str().into(),
        min_memory: s.java.min_memory_mb.to_string().into(),
        max_memory: s.java.max_memory_mb.to_string().into(),
        jvm_args: s.java.jvm_args.as_str().into(),
        width: s.window.width.to_string().into(),
        height: s.window.height.to_string().into(),
        fullscreen: s.window.fullscreen,
        downloads: s.concurrent_downloads.to_string().into(),
        hide_on_launch: s.hide_on_launch,
        auto_update: s.auto_update,
        curseforge_key: s.curseforge_api_key.as_str().into(),
        msa_client_id: s.microsoft_client_id.as_str().into(),
        data_dir: paths.root().to_string_lossy().as_ref().into(),
    }
}

pub fn apply_global_settings(data: &GlobalSettingsData, s: &mut GlobalSettings) -> Result<(), String> {
    let (min, max) = parse_memory(&data.min_memory, &data.max_memory)?;
    let window = parse_window(&data.width, &data.height, data.fullscreen)?;
    let downloads = parse_number("Parallel downloads", &data.downloads, 1, 64)?;

    s.theme = match data.theme {
        1 => Theme::Light,
        2 => Theme::Dark,
        _ => Theme::System,
    };
    s.java.path = data.java_path.trim().to_string();
    s.java.min_memory_mb = min;
    s.java.max_memory_mb = max;
    s.java.jvm_args = data.jvm_args.trim().to_string();
    s.window = window;
    s.concurrent_downloads = downloads as usize;
    s.hide_on_launch = data.hide_on_launch;
    s.auto_update = data.auto_update;
    s.curseforge_api_key = data.curseforge_key.trim().to_string();
    s.microsoft_client_id = data.msa_client_id.trim().to_string();
    Ok(())
}

fn parse_number(field: &str, value: &str, min: u32, max: u32) -> Result<u32, String> {
    match value.trim().parse::<u32>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(format!("{field} must be a whole number between {min} and {max}")),
    }
}

fn parse_memory(min: &str, max: &str) -> Result<(u32, u32), String> {
    let min = parse_number("Minimum memory", min, 256, 65_536)?;
    let max = parse_number("Maximum memory", max, 512, 65_536)?;
    if min > max {
        return Err("Minimum memory cannot be larger than maximum memory".into());
    }
    Ok((min, max))
}

fn parse_window(width: &str, height: &str, fullscreen: bool) -> Result<WindowSettings, String> {
    Ok(WindowSettings {
        width: parse_number("Window width", width, 320, 16_384)?,
        height: parse_number("Window height", height, 240, 16_384)?,
        fullscreen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance() -> Instance {
        Instance {
            id: "test".into(),
            dir: "C:/x".into(),
            config: InstanceConfig { name: "Test".into(), game_version: "1.21.10".into(), ..Default::default() },
        }
    }

    #[test]
    fn instance_settings_round_trip() {
        let global = GlobalSettings::default();
        let mut inst = instance();
        let mut data = instance_settings(&inst, &global);
        assert!(!data.memory_override);
        assert_eq!(data.max_memory, global.java.max_memory_mb.to_string());

        data.memory_override = true;
        data.min_memory = "1024".into();
        data.max_memory = "6144".into();
        data.java_override = true;
        data.java_path = "".into();
        apply_instance_settings(&data, &mut inst.config).unwrap();
        assert_eq!(inst.config.overrides.memory, Some(MemoryOverride { min_mb: 1024, max_mb: 6144 }));
        assert_eq!(inst.config.overrides.java_path.as_deref(), Some(""));
        assert_eq!(instance_settings(&inst, &global), data);
    }

    #[test]
    fn rejects_bad_numbers() {
        let mut inst = instance();
        let mut data = instance_settings(&inst, &GlobalSettings::default());
        data.memory_override = true;
        data.min_memory = "8000".into();
        data.max_memory = "4000".into();
        assert!(apply_instance_settings(&data, &mut inst.config).is_err());
        data.min_memory = "abc".into();
        assert!(apply_instance_settings(&data, &mut inst.config).is_err());
    }

    #[test]
    fn global_settings_round_trip() {
        let paths = Paths::new("C:/data");
        let mut s = GlobalSettings::default();
        let mut data = global_settings(&s, &paths);
        data.msa_client_id = " id ".into();
        data.theme = 1;
        data.downloads = "8".into();
        data.curseforge_key = "abc".into();
        apply_global_settings(&data, &mut s).unwrap();
        assert_eq!(s.theme, Theme::Light);
        assert_eq!(s.concurrent_downloads, 8);
        assert_eq!(s.curseforge_api_key, "abc");
        assert_eq!(s.microsoft_client_id, "id");
        data.msa_client_id = "id".into();
        assert_eq!(global_settings(&s, &paths), data);

        data.downloads = "0".into();
        assert!(apply_global_settings(&data, &mut s).is_err());
    }
}

#[cfg(test)]
mod loader_tests {
    use super::*;

    #[test]
    fn loader_indices() {
        for loader in ModLoader::ALL {
            assert_eq!(loader_from_index(loader_index(loader)), loader);
        }
        assert_eq!(loader_from_index(99), ModLoader::Vanilla);
        assert_eq!(kind_from_index(2), ContentKind::Shader);
        assert_eq!(kind_from_index(-1), ContentKind::Mod);
    }
}
