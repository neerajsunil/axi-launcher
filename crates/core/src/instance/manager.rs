use std::fs;
use std::path::PathBuf;

use super::model::{CONFIG_FILE, Instance, InstanceConfig};
use crate::loader::ModLoader;
use crate::util::copy_dir_all;
use crate::{Error, Result};

/// The game an instance runs: Minecraft version plus mod loader.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameSetup {
    pub version: String,
    pub loader: ModLoader,
    pub loader_version: String,
}

impl GameSetup {
    pub fn vanilla(version: &str) -> Self {
        Self { version: version.into(), ..Default::default() }
    }
}

/// Creates, lists and removes instances inside one directory.
#[derive(Debug, Clone)]
pub struct InstanceManager {
    root: PathBuf,
}

impl InstanceManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// All valid instances, sorted by name. Folders without a readable config are skipped.
    pub fn list(&self) -> Result<Vec<Instance>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut instances: Vec<Instance> = entries
            .flatten()
            .filter(|e| e.path().join(CONFIG_FILE).is_file())
            .filter_map(|e| self.get(&e.file_name().to_string_lossy()).ok())
            .collect();
        instances.sort_by_key(|i| i.config.name.to_lowercase());
        Ok(instances)
    }

    pub fn get(&self, id: &str) -> Result<Instance> {
        let dir = self.dir_of(id)?;
        let text = fs::read_to_string(Instance::config_path(&dir))
            .map_err(|e| Error::msg(format!("instance '{id}' not found: {e}")))?;
        Ok(Instance { id: id.to_string(), dir, config: toml::from_str(&text)? })
    }

    pub fn create(&self, name: &str, group: &str, game: GameSetup) -> Result<Instance> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::msg("Instance name cannot be empty"));
        }
        let id = self.unique_id(name);
        let dir = self.root.join(&id);
        let mut instance = Instance {
            id,
            dir,
            config: InstanceConfig {
                name: name.to_string(),
                game_version: game.version,
                loader: game.loader,
                loader_version: game.loader_version,
                group: group.trim().to_string(),
                ..Default::default()
            },
        };
        instance.touch_created();
        fs::create_dir_all(instance.game_dir())?;
        instance.save()?;
        Ok(instance)
    }

    /// Duplicates an instance including its worlds and files. Play statistics start fresh.
    pub fn copy(&self, id: &str, new_name: &str) -> Result<Instance> {
        let source = self.get(id)?;
        let new_id = self.unique_id(new_name);
        let dir = self.root.join(&new_id);
        copy_dir_all(&source.dir, &dir)?;
        let mut copy = Instance { id: new_id, dir, config: source.config };
        copy.config.name = new_name.to_string();
        copy.config.last_played = None;
        copy.config.total_playtime_secs = 0;
        copy.touch_created();
        let _ = fs::remove_file(copy.log_file());
        copy.save()?;
        Ok(copy)
    }

    pub fn rename(&self, id: &str, new_name: &str) -> Result<Instance> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err(Error::msg("Instance name cannot be empty"));
        }
        let mut instance = self.get(id)?;
        instance.config.name = new_name.to_string();
        instance.save()?;
        Ok(instance)
    }

    /// Permanently deletes the instance folder (shared libraries and assets are kept).
    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.dir_of(id)?;
        if !dir.join(CONFIG_FILE).is_file() {
            return Err(Error::msg(format!("'{id}' is not an instance")));
        }
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// Instance names currently in use.
    pub fn names(&self) -> Vec<String> {
        self.list().unwrap_or_default().into_iter().map(|i| i.config.name).collect()
    }

    fn dir_of(&self, id: &str) -> Result<PathBuf> {
        let valid = !id.is_empty()
            && id != "."
            && id != ".."
            && !id.contains(['/', '\\', ':']);
        if !valid {
            return Err(Error::msg(format!("invalid instance id '{id}'")));
        }
        Ok(self.root.join(id))
    }

    fn unique_id(&self, name: &str) -> String {
        let base = slugify(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.root.join(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

/// Folder-safe version of a display name.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches(['-', '.']).to_string();
    if slug.is_empty() { "instance".into() } else { slug }
}

/// `base`, or `base (2)`, `base (3)`, ... whichever is not taken.
pub fn unique_name(taken: &[String], base: &str) -> String {
    let base = base.trim();
    let is_taken = |n: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    if !is_taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} ({n})")).find(|n| !is_taken(n)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> (tempfile::TempDir, InstanceManager) {
        let dir = tempfile::tempdir().unwrap();
        let m = InstanceManager::new(dir.path().join("instances"));
        (dir, m)
    }

    #[test]
    fn create_list_get() {
        let (_tmp, m) = manager();
        assert!(m.list().unwrap().is_empty());
        let a = m.create("My World", "Survival", GameSetup::vanilla("1.21.10")).unwrap();
        assert_eq!(a.id, "my-world");
        assert!(a.game_dir().is_dir());
        m.create("Another", "", GameSetup::vanilla("1.20.1")).unwrap();

        let list = m.list().unwrap();
        assert_eq!(list.iter().map(|i| i.config.name.as_str()).collect::<Vec<_>>(), ["Another", "My World"]);
        assert_eq!(m.get("my-world").unwrap(), a);
    }

    #[test]
    fn duplicate_names_get_unique_ids() {
        let (_tmp, m) = manager();
        let a = m.create("Test", "", GameSetup::vanilla("1.21.10")).unwrap();
        let b = m.create("Test", "", GameSetup::vanilla("1.21.10")).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(b.id, "test-2");
    }

    #[test]
    fn rename_keeps_folder() {
        let (_tmp, m) = manager();
        let a = m.create("Old", "", GameSetup::vanilla("1.21.10")).unwrap();
        let renamed = m.rename(&a.id, "New name").unwrap();
        assert_eq!(renamed.id, a.id);
        assert_eq!(m.get(&a.id).unwrap().config.name, "New name");
        assert!(m.rename(&a.id, "  ").is_err());
    }

    #[test]
    fn copy_duplicates_files_and_resets_stats() {
        let (_tmp, m) = manager();
        let mut a = m.create("Original", "", GameSetup::vanilla("1.21.10")).unwrap();
        a.record_session(100, 400);
        a.save().unwrap();
        fs::write(a.game_dir().join("options.txt"), "fov:90").unwrap();

        let b = m.copy(&a.id, "Original (2)").unwrap();
        assert_ne!(b.id, a.id);
        assert_eq!(b.config.game_version, "1.21.10");
        assert_eq!(b.config.total_playtime_secs, 0);
        assert_eq!(fs::read_to_string(b.game_dir().join("options.txt")).unwrap(), "fov:90");
        assert_eq!(m.get(&a.id).unwrap().config.total_playtime_secs, 300);
    }

    #[test]
    fn delete_removes_folder_and_rejects_bad_ids() {
        let (_tmp, m) = manager();
        let a = m.create("Gone", "", GameSetup::vanilla("1.21.10")).unwrap();
        m.delete(&a.id).unwrap();
        assert!(!a.dir.exists());
        assert!(m.delete("..").is_err());
        assert!(m.delete("a/b").is_err());
        assert!(m.delete("missing").is_err());
    }

    #[test]
    fn names() {
        assert_eq!(slugify("Hello, World! 1.21"), "hello-world-1.21");
        assert_eq!(slugify("???"), "instance");
        let taken = vec!["1.21.10".to_string(), "1.21.10 (2)".to_string()];
        assert_eq!(unique_name(&taken, "1.21.10"), "1.21.10 (3)");
        assert_eq!(unique_name(&taken, "Fresh"), "Fresh");
    }
}
