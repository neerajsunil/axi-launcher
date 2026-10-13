//! Mod loaders: which ones exist, their versions, and installing them.

mod fabric;
mod neoforge;

use std::path::Path;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::launch::OutputSink;
use crate::meta::version::LoaderProfile;
use crate::paths::Paths;
use crate::{Progress, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModLoader {
    #[default]
    Vanilla,
    Fabric,
    NeoForge,
}

impl ModLoader {
    pub const ALL: [ModLoader; 3] = [ModLoader::Vanilla, ModLoader::Fabric, ModLoader::NeoForge];

    pub fn display_name(self) -> &'static str {
        match self {
            ModLoader::Vanilla => "Vanilla",
            ModLoader::Fabric => "Fabric",
            ModLoader::NeoForge => "NeoForge",
        }
    }

    /// Whether this loader can run mods.
    pub fn is_modded(self) -> bool {
        self != ModLoader::Vanilla
    }
}

/// One release of a loader for a particular game version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

/// Versions of `loader` that support `game_version`, newest first. Empty if unsupported.
pub fn fetch_versions(client: &Client, loader: ModLoader, game_version: &str) -> Result<Vec<LoaderVersion>> {
    match loader {
        ModLoader::Vanilla => Ok(Vec::new()),
        ModLoader::Fabric => fabric::versions(client, game_version),
        ModLoader::NeoForge => neoforge::versions(client, game_version),
    }
}

/// The version to pick by default: the newest stable one, else the newest.
pub fn recommended(versions: &[LoaderVersion]) -> Option<&LoaderVersion> {
    versions.iter().find(|v| v.stable).or(versions.first())
}

/// Everything a loader install needs.
pub(crate) struct InstallContext<'a> {
    pub client: &'a Client,
    pub paths: &'a Paths,
    /// Java used to run installers.
    pub java: &'a Path,
    pub progress: Progress<'a>,
    pub output: &'a OutputSink,
}

/// Installs `loader` `version` for `game_version` (once) and returns its profile.
pub(crate) fn install(
    ctx: &InstallContext,
    loader: ModLoader,
    game_version: &str,
    version: &str,
) -> Result<Option<LoaderProfile>> {
    match loader {
        ModLoader::Vanilla => Ok(None),
        ModLoader::Fabric => fabric::install(ctx, game_version, version).map(Some),
        ModLoader::NeoForge => neoforge::install(ctx, game_version, version).map(Some),
    }
}
