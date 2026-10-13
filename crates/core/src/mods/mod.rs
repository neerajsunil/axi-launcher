//! Projects from Modrinth and CurseForge (mods, resource packs, shaders, modpacks):
//! searching them, and the ones installed in an instance.

pub(crate) mod curseforge;
pub mod description;
mod installed;
mod kind;
pub(crate) mod modrinth;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

pub use installed::{
    ContentEntry, InstallReport, InstallRequest, InstalledContent, check_updates, has_shader_loader, install, list_installed, record, remove, set_enabled,
};
pub use kind::{ContentKind, IRIS_MODRINTH_ID};

use crate::loader::ModLoader;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModSource {
    #[default]
    Modrinth,
    CurseForge,
}

impl ModSource {
    pub fn display_name(self) -> &'static str {
        match self {
            ModSource::Modrinth => "Modrinth",
            ModSource::CurseForge => "CurseForge",
        }
    }
}

/// What to search for and which game the results must support.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub text: String,
    pub kind: ContentKind,
    /// Empty for any version (modpacks).
    pub game_version: String,
    /// Only used for mods.
    pub loader: ModLoader,
    pub offset: u32,
    pub limit: u32,
}

/// A project in search results.
#[derive(Debug, Clone, PartialEq)]
pub struct ModHit {
    pub source: ModSource,
    pub project_id: String,
    pub title: String,
    pub author: String,
    pub description: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub page_url: String,
}

#[derive(Debug, Clone)]
pub struct SearchPage {
    pub hits: Vec<ModHit>,
    pub total: u32,
}

/// A downloadable file of a project, compatible with the requested game.
#[derive(Debug, Clone, PartialEq)]
pub struct ModFile {
    pub source: ModSource,
    pub project_id: String,
    /// Modrinth version id or CurseForge file id.
    pub file_id: String,
    pub version_number: String,
    pub file_name: String,
    /// `None` when the author doesn't allow third-party downloads (CurseForge).
    pub url: Option<String>,
    pub size: Option<u64>,
    /// Project ids of required dependencies.
    pub dependencies: Vec<String>,
}

/// A version of a project, for picking one by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectVersion {
    /// Modrinth version id or CurseForge file id.
    pub file_id: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    /// Lowercase loader ids ("fabric", "neoforge", "iris", ...).
    pub loaders: Vec<String>,
    /// A release rather than a beta or alpha.
    pub stable: bool,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Made for the instance's game version (and loader, where that matters).
    pub compatible: bool,
}

/// Title and icon of a project.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub title: String,
    pub icon_url: Option<String>,
}

/// Everything shown on a project's page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectDetails {
    pub title: String,
    pub summary: String,
    pub icon_url: Option<String>,
    pub body: Vec<description::Block>,
    pub downloads: u64,
    pub followers: u64,
    pub categories: Vec<String>,
    /// "required", "optional" or "unsupported".
    pub client_side: String,
    pub server_side: String,
    pub license: String,
    /// `YYYY-MM-DD`.
    pub published: String,
    pub updated: String,
    /// (label, URL) of the source code, issues, wiki, Discord and donation pages.
    pub links: Vec<(String, String)>,
    pub gallery: Vec<GalleryImage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GalleryImage {
    pub url: String,
    pub title: String,
}

/// Access to the mod sites. CurseForge needs an API key (console.curseforge.com).
pub struct ModApi<'a> {
    pub client: &'a Client,
    pub curseforge_key: &'a str,
}

impl ModApi<'_> {
    pub fn search(&self, source: ModSource, query: &SearchQuery) -> Result<SearchPage> {
        match source {
            ModSource::Modrinth => modrinth::search(self.client, query),
            ModSource::CurseForge => curseforge::search(self.client, self.key()?, query),
        }
    }

    /// The newest file of `project_id` for this game version (any if empty) and loader, if any.
    pub fn latest_file(
        &self,
        source: ModSource,
        kind: ContentKind,
        project_id: &str,
        game_version: &str,
        loader: ModLoader,
    ) -> Result<Option<ModFile>> {
        match source {
            ModSource::Modrinth => modrinth::latest_file(self.client, kind, project_id, game_version, loader),
            ModSource::CurseForge => {
                curseforge::latest_file(self.client, self.key()?, kind, project_id, game_version, loader)
            }
        }
    }

    /// A specific file of a project.
    pub fn file(&self, source: ModSource, kind: ContentKind, project_id: &str, file_id: &str) -> Result<Option<ModFile>> {
        match source {
            ModSource::Modrinth => modrinth::file(self.client, kind, file_id),
            ModSource::CurseForge => curseforge::file(self.client, self.key()?, kind, project_id, file_id),
        }
    }

    /// All versions of a project, newest first, marked by whether they suit this game version and loader.
    pub fn versions(
        &self,
        source: ModSource,
        kind: ContentKind,
        project_id: &str,
        game_version: &str,
        loader: ModLoader,
    ) -> Result<Vec<ProjectVersion>> {
        match source {
            ModSource::Modrinth => modrinth::versions(self.client, kind, project_id, game_version, loader),
            ModSource::CurseForge => curseforge::versions(self.client, self.key()?, kind, project_id, game_version, loader),
        }
    }

    pub fn project(&self, source: ModSource, project_id: &str) -> Result<ProjectInfo> {
        match source {
            ModSource::Modrinth => modrinth::project(self.client, project_id),
            ModSource::CurseForge => curseforge::project(self.client, self.key()?, project_id),
        }
    }

    /// The full page of a Modrinth project.
    pub fn project_details(&self, project_id: &str) -> Result<ProjectDetails> {
        modrinth::details(self.client, project_id)
    }

    pub(crate) fn key(&self) -> Result<&str> {
        let key = self.curseforge_key.trim();
        if key.is_empty() {
            return Err(Error::msg("CurseForge needs an API key. Add one in Settings."));
        }
        Ok(key)
    }
}
