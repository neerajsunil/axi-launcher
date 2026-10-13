use std::path::{Path, PathBuf};

/// Layout of the launcher's data directory.
///
/// ```text
/// <root>/settings.toml
/// <root>/accounts.json
/// <root>/instances/<id>/...
/// <root>/libraries, assets, versions, runtime   (shared by all instances)
/// <root>/cache/
/// ```
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `%APPDATA%\.axi-launcher` on Windows, `~/.axi-launcher` elsewhere.
    pub fn default_root() -> PathBuf {
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let root = base.join(".axi-launcher");
        // Carry data over from before the rename to Axi. If it can't move yet
        // (a file is in use), keep using it and try again next launch.
        let legacy = base.join(".mc-launcher");
        if !root.exists() && legacy.is_dir() && std::fs::rename(&legacy, &root).is_err() {
            return legacy;
        }
        root
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.toml")
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }

    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions_dir().join(id)
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }
}
