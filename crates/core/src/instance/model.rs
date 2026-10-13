use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::loader::ModLoader;
use crate::mods::{ContentKind, ModSource};
use crate::settings::WindowSettings;
use crate::util::{now_unix, write_atomic};

pub(super) const CONFIG_FILE: &str = "instance.toml";

/// Per-instance settings. `None` means "use the global setting".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overrides {
    /// Java executable; an empty string forces Mojang's bundled runtime.
    pub java_path: Option<String>,
    pub memory: Option<MemoryOverride>,
    pub jvm_args: Option<String>,
    pub window: Option<WindowSettings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryOverride {
    pub min_mb: u32,
    pub max_mb: u32,
}

/// Contents of `instance.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstanceConfig {
    pub name: String,
    pub game_version: String,
    pub loader: ModLoader,
    /// Loader release, e.g. `0.17.3`; empty for vanilla (or "newest" on first launch).
    pub loader_version: String,
    pub group: String,
    pub notes: String,
    /// Unix seconds.
    pub created: u64,
    pub last_played: Option<u64>,
    pub total_playtime_secs: u64,
    pub overrides: Overrides,
    /// The modpack the instance was installed from.
    pub pack: Option<PackOrigin>,
}

/// A modpack an instance came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PackOrigin {
    pub name: String,
    pub version: String,
    /// `None` when imported from a file.
    pub source: Option<ModSource>,
    pub project_id: String,
    pub file_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// Folder name; stable for the instance's lifetime (renaming only changes `config.name`).
    pub id: String,
    pub dir: PathBuf,
    pub config: InstanceConfig,
}

impl Instance {
    /// The game's working directory (saves, mods, options.txt, ...).
    pub fn game_dir(&self) -> PathBuf {
        self.dir.join(".minecraft")
    }

    /// Output captured from the game's last run.
    pub fn log_file(&self) -> PathBuf {
        self.dir.join("launcher.log")
    }

    /// The picture shown for the instance; when missing, the UI shows its loader's logo.
    pub fn icon_file(&self) -> PathBuf {
        self.dir.join("icon.png")
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.content_dir(ContentKind::Mod)
    }

    /// Where files of `kind` go (`mods`, `resourcepacks`, `shaderpacks`).
    pub fn content_dir(&self, kind: ContentKind) -> PathBuf {
        self.game_dir().join(kind.folder())
    }

    pub(super) fn config_path(dir: &Path) -> PathBuf {
        dir.join(CONFIG_FILE)
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(&Self::config_path(&self.dir), toml::to_string_pretty(&self.config)?)?;
        Ok(())
    }

    /// Records a finished play session.
    pub fn record_session(&mut self, started: u64, ended: u64) {
        self.config.last_played = Some(ended);
        self.config.total_playtime_secs += ended.saturating_sub(started);
    }

    pub fn touch_created(&mut self) {
        self.config.created = now_unix();
    }
}
