//! Which files are synced. Paths are relative to the instance folder and `/`-separated.
//!
//! Only what is shared between computers syncs: content, worlds and the server list.
//! Mod configs and game settings (`config/`, `options.txt`, ...) stay per computer.

use std::path::{Component, Path, PathBuf};

use crate::{Error, Result};

/// Folders inside `.minecraft` that sync with everything in them.
const FOLDERS: [&str; 4] = ["mods", "resourcepacks", "shaderpacks", "saves"];
/// Single files inside `.minecraft` that sync.
const FILES: [&str; 1] = ["servers.dat"];
/// Files inside the instance folder (next to `.minecraft`) that sync.
const INSTANCE_FILES: [&str; 2] = ["content.toml", "icon.png"];

/// Written by the game or the launcher while working; never complete enough to copy.
fn is_scratch(name: &str) -> bool {
    name == "session.lock" || name.ends_with(".tmp") || name.ends_with(".part") || name.ends_with(".axi-part")
}

/// Whether `path` is synced.
pub fn is_synced(path: &str) -> bool {
    if INSTANCE_FILES.contains(&path) {
        return true;
    }
    let Some(rest) = path.strip_prefix(".minecraft/") else { return false };
    let name = rest.rsplit('/').next().unwrap_or(rest);
    if is_scratch(name) {
        return false;
    }
    match rest.split_once('/') {
        Some((folder, _)) => FOLDERS.contains(&folder),
        None => FILES.contains(&rest),
    }
}

/// The world folder `path` belongs to (`.minecraft/saves/<world>`), if any.
pub fn world_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(".minecraft/saves/")?;
    let (world, _) = rest.split_once('/')?;
    Some(&path[..".minecraft/saves/".len() + world.len()])
}

/// Rejects paths from another computer that could point outside the instance or aren't synced.
pub fn check(path: &str) -> Result<()> {
    let bad = path.is_empty()
        || path.contains(['\\', ':', '\0'])
        || path.starts_with('/')
        || path.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        || !is_synced(path);
    if bad {
        return Err(Error::msg(format!("refusing to sync '{path}'")));
    }
    Ok(())
}

/// Where `path` lives on disk. `path` must have passed [`check`].
pub fn on_disk(instance_dir: &Path, path: &str) -> PathBuf {
    let mut full = instance_dir.to_path_buf();
    full.extend(path.split('/'));
    full
}

/// The `/`-separated form of `full`, relative to `instance_dir`.
pub fn relative(instance_dir: &Path, full: &Path) -> Option<String> {
    let rel = full.strip_prefix(instance_dir).ok()?;
    let parts: Option<Vec<&str>> = rel
        .components()
        .map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    Some(parts?.join("/"))
}

/// True when `full` or a folder between it and `instance_dir` is a link, which a peer
/// must never be able to follow out of the instance.
pub fn crosses_link(instance_dir: &Path, full: &Path) -> bool {
    let mut current = full;
    while current != instance_dir {
        if std::fs::symlink_metadata(current).is_ok_and(|m| m.file_type().is_symlink()) {
            return true;
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return true,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_paths() {
        for yes in [
            "content.toml",
            "icon.png",
            ".minecraft/mods/sodium.jar",
            ".minecraft/mods/sodium.jar.disabled",
            ".minecraft/resourcepacks/x.zip",
            ".minecraft/shaderpacks/c.zip",
            ".minecraft/saves/World/level.dat",
            ".minecraft/saves/World/region/r.0.0.mca",
            ".minecraft/servers.dat",
        ] {
            assert!(is_synced(yes), "{yes}");
        }
        for no in [
            "instance.toml",
            "launcher.log",
            ".minecraft/options.txt",
            ".minecraft/config/sodium.json",
            ".minecraft/logs/latest.log",
            ".minecraft/screenshots/a.png",
            ".minecraft/saves/World/session.lock",
            ".minecraft/mods/a.jar.part",
            ".minecraft/mods",
            "sync/base.json",
        ] {
            assert!(!is_synced(no), "{no}");
        }
    }

    #[test]
    fn worlds() {
        assert_eq!(world_of(".minecraft/saves/My World/level.dat"), Some(".minecraft/saves/My World"));
        assert_eq!(world_of(".minecraft/saves/x/region/r.mca"), Some(".minecraft/saves/x"));
        assert_eq!(world_of(".minecraft/mods/a.jar"), None);
    }

    #[test]
    fn unsafe_paths() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            ".minecraft/saves/../../x",
            ".minecraft/saves/a//b",
            ".minecraft\\mods\\a.jar",
            "C:/x",
            ".minecraft/config/a.json",
        ] {
            assert!(check(bad).is_err(), "{bad}");
        }
        assert!(check(".minecraft/saves/World/level.dat").is_ok());
    }

    #[test]
    fn disk_paths() {
        let dir = Path::new("inst");
        let full = on_disk(dir, ".minecraft/saves/W/level.dat");
        assert_eq!(full, Path::new("inst").join(".minecraft").join("saves").join("W").join("level.dat"));
        assert_eq!(relative(dir, &full).as_deref(), Some(".minecraft/saves/W/level.dat"));
    }
}
