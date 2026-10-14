//! Launcher-wide settings, stored in `settings.toml`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::util::write_atomic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system's light/dark setting.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JavaSettings {
    /// Path to a java executable; empty means "download the version the game needs".
    pub path: String,
    pub min_memory_mb: u32,
    pub max_memory_mb: u32,
    /// Extra JVM arguments, separated by whitespace.
    pub jvm_args: String,
}

impl Default for JavaSettings {
    fn default() -> Self {
        Self {
            path: String::new(),
            min_memory_mb: 512,
            max_memory_mb: 4096,
            jvm_args: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self { width: 854, height: 480, fullscreen: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    pub theme: Theme,
    pub java: JavaSettings,
    pub window: WindowSettings,
    pub concurrent_downloads: usize,
    /// Hide the launcher while a game is running and bring it back when it exits.
    pub hide_on_launch: bool,
    /// The user's own key from console.curseforge.com; empty means the built-in one.
    pub curseforge_api_key: String,
    /// Azure app used for Microsoft sign-in; empty means the one built into the launcher.
    pub microsoft_client_id: String,
    /// Identifies this launcher to others on the network for LAN sync; made on first run.
    pub device_id: String,
    /// Download new launcher versions in the background and install them on restart.
    pub auto_update: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            java: JavaSettings::default(),
            window: WindowSettings::default(),
            concurrent_downloads: 16,
            hide_on_launch: false,
            curseforge_api_key: String::new(),
            microsoft_client_id: String::new(),
            device_id: String::new(),
            auto_update: true,
        }
    }
}

/// CurseForge API key compiled in through the `AXI_CURSEFORGE_API_KEY` environment variable.
pub const BUILT_IN_CURSEFORGE_KEY: Option<&str> = option_env!("AXI_CURSEFORGE_API_KEY");

impl GlobalSettings {
    /// The CurseForge key to use: the user's own, else the built-in one; empty if there is neither.
    pub fn curseforge_key(&self) -> String {
        let own = self.curseforge_api_key.trim();
        if own.is_empty() { BUILT_IN_CURSEFORGE_KEY.unwrap_or_default().trim().to_string() } else { own.to_string() }
    }

    /// Loads settings, falling back to defaults when the file does not exist yet.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// The offline name older versions kept in `settings.toml`, before accounts existed.
pub fn legacy_username(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("username")?.as_str().map(str::to_string)
}

/// Checks a value the user typed as an offline username.
pub fn validate_username(name: &str) -> std::result::Result<(), &'static str> {
    if !(3..=16).contains(&name.len()) {
        return Err("Username must be 3-16 characters long");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("Username may only contain letters, digits and _");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let s = GlobalSettings::load(&dir.path().join("settings.toml")).unwrap();
        assert_eq!(s, GlobalSettings::default());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut s = GlobalSettings::default();
        s.microsoft_client_id = "abc".into();
        s.theme = Theme::Light;
        s.java.max_memory_mb = 6144;
        s.java.jvm_args = "-XX:+UseG1GC".into();
        s.window.fullscreen = true;
        s.save(&path).unwrap();
        assert_eq!(GlobalSettings::load(&path).unwrap(), s);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "username = \"Alex\"\n[java]\nmax_memory_mb = 2048\n").unwrap();
        let s = GlobalSettings::load(&path).unwrap();
        assert_eq!(legacy_username(&path).as_deref(), Some("Alex"));
        assert_eq!(s.java.max_memory_mb, 2048);
        assert_eq!(s.java.min_memory_mb, JavaSettings::default().min_memory_mb);
    }

    #[test]
    fn own_curseforge_key_wins() {
        let mut s = GlobalSettings::default();
        assert_eq!(s.curseforge_key(), BUILT_IN_CURSEFORGE_KEY.unwrap_or_default());
        s.curseforge_api_key = " mine ".into();
        assert_eq!(s.curseforge_key(), "mine");
    }

    #[test]
    fn no_legacy_username() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert_eq!(legacy_username(&path), None);
        GlobalSettings::default().save(&path).unwrap();
        assert_eq!(legacy_username(&path), None);
    }

    #[test]
    fn usernames() {
        assert!(validate_username("Steve_01").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("has space").is_err());
        assert!(validate_username("waytoolongusername").is_err());
    }
}
