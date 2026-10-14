//! CurseForge API v1 (https://docs.curseforge.com). Needs an API key.

use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{ContentKind, ModFile, ModHit, ModSource, ProjectInfo, ProjectVersion, SearchPage, SearchQuery};
use crate::loader::ModLoader;
use crate::{Error, Result};

const API: &str = "https://api.curseforge.com/v1";
const MINECRAFT: &str = "432";
/// `relationType` of a required dependency.
const REQUIRED: u32 = 3;
/// `releaseType` of a stable release.
const RELEASE: u32 = 1;

fn get<T: DeserializeOwned>(client: &Client, key: &str, path: &str, query: &[(&str, String)]) -> Result<T> {
    read(client.get(format!("{API}{path}")).header("x-api-key", key).query(query).send()?)
}

fn post<T: DeserializeOwned>(client: &Client, key: &str, path: &str, body: &serde_json::Value) -> Result<T> {
    read(client.post(format!("{API}{path}")).header("x-api-key", key).json(body).send()?)
}

fn read<T: DeserializeOwned>(resp: reqwest::blocking::Response) -> Result<T> {
    if resp.status() == StatusCode::FORBIDDEN || resp.status() == StatusCode::UNAUTHORIZED {
        return Err(Error::msg("Your CurseForge key doesn't work. Check it in Settings."));
    }
    Ok(resp.error_for_status()?.json()?)
}

fn loader_type(loader: ModLoader) -> &'static str {
    match loader {
        ModLoader::Vanilla => "0",
        ModLoader::Fabric => "4",
        ModLoader::NeoForge => "6",
    }
}

#[derive(Deserialize)]
struct Response<T> {
    data: T,
    pagination: Option<Pagination>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    total_count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mod {
    id: u64,
    name: String,
    #[serde(default)]
    class_id: Option<u32>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    download_count: f64,
    logo: Option<Logo>,
    #[serde(default)]
    authors: Vec<Author>,
    links: Option<Links>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Logo {
    thumbnail_url: Option<String>,
}

#[derive(Deserialize)]
struct Author {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Links {
    website_url: Option<String>,
}

fn icon(logo: Option<Logo>) -> Option<String> {
    logo.and_then(|l| l.thumbnail_url).filter(|u| !u.is_empty())
}

/// `gameVersion` and `modLoaderType` filters; the loader only matters for mods.
fn game_filter(kind: ContentKind, game_version: &str, loader: ModLoader) -> Vec<(&'static str, String)> {
    let mut query = Vec::new();
    if !game_version.is_empty() {
        query.push(("gameVersion", game_version.to_string()));
    }
    if kind == ContentKind::Mod {
        query.push(("modLoaderType", loader_type(loader).into()));
    }
    query
}

pub fn search(client: &Client, key: &str, q: &SearchQuery) -> Result<SearchPage> {
    let mut query = vec![
        ("gameId", MINECRAFT.to_string()),
        ("classId", q.kind.curseforge_class().to_string()),
        ("searchFilter", q.text.trim().to_string()),
        // Popularity; CurseForge has no relevance sort.
        ("sortField", "2".into()),
        ("sortOrder", "desc".into()),
        ("index", q.offset.to_string()),
        ("pageSize", q.limit.to_string()),
    ];
    query.extend(game_filter(q.kind, &q.game_version, q.loader));
    let resp: Response<Vec<Mod>> = get(client, key, "/mods/search", &query)?;
    Ok(SearchPage {
        total: resp.pagination.map_or(0, |p| p.total_count),
        hits: resp
            .data
            .into_iter()
            .map(|m| ModHit {
                source: ModSource::CurseForge,
                project_id: m.id.to_string(),
                author: m.authors.into_iter().next().map(|a| a.name).unwrap_or_default(),
                description: m.summary,
                downloads: m.download_count as u64,
                page_url: m.links.and_then(|l| l.website_url).unwrap_or_default(),
                icon_url: icon(m.logo),
                title: m.name,
            })
            .collect(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    id: u64,
    mod_id: u64,
    display_name: String,
    file_name: String,
    release_type: u32,
    file_date: String,
    file_length: Option<u64>,
    download_url: Option<String>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
    /// Game versions mixed with loader names ("1.21.1", "Fabric", "Client", ...).
    #[serde(default)]
    game_versions: Vec<String>,
}

/// Loader names CurseForge puts in `gameVersions`.
const LOADER_NAMES: [&str; 4] = ["Fabric", "NeoForge", "Forge", "Quilt"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dependency {
    mod_id: u64,
    relation_type: u32,
}

pub fn latest_file(
    client: &Client,
    key: &str,
    kind: ContentKind,
    project_id: &str,
    game_version: &str,
    loader: ModLoader,
) -> Result<Option<ModFile>> {
    let mut query = vec![("pageSize", "50".to_string())];
    query.extend(game_filter(kind, game_version, loader));
    let resp: Response<Vec<File>> = get(client, key, &format!("/mods/{project_id}/files"), &query)?;
    let mut files = resp.data;
    files.sort_by(|a, b| b.file_date.cmp(&a.file_date));
    let pick = files.iter().position(|f| f.release_type == RELEASE).or((!files.is_empty()).then_some(0));
    Ok(pick.map(|i| to_mod_file(kind, files.swap_remove(i))))
}

/// One specific file of a project.
pub fn file(client: &Client, key: &str, kind: ContentKind, project_id: &str, file_id: &str) -> Result<Option<ModFile>> {
    let resp: Response<File> = get(client, key, &format!("/mods/{project_id}/files/{file_id}"), &[])?;
    Ok(Some(to_mod_file(kind, resp.data)))
}

/// The project's files (up to 200), newest first, marked by whether they suit the instance.
pub fn versions(
    client: &Client,
    key: &str,
    kind: ContentKind,
    project_id: &str,
    game_version: &str,
    loader: ModLoader,
) -> Result<Vec<ProjectVersion>> {
    let mut files = Vec::new();
    for page in 0..4 {
        let query = [("index", (page * 50).to_string()), ("pageSize", "50".into())];
        let resp: Response<Vec<File>> = get(client, key, &format!("/mods/{project_id}/files"), &query)?;
        let done = resp.data.len() < 50;
        files.extend(resp.data);
        if done {
            break;
        }
    }
    files.sort_by(|a, b| b.file_date.cmp(&a.file_date));
    Ok(files
        .into_iter()
        .map(|f| {
            let (loaders, game_versions): (Vec<String>, Vec<String>) =
                f.game_versions.into_iter().partition(|v| LOADER_NAMES.contains(&v.as_str()));
            let game_versions: Vec<String> =
                game_versions.into_iter().filter(|v| v.starts_with(|c: char| c.is_ascii_digit())).collect();
            ProjectVersion {
                compatible: game_versions.iter().any(|g| g == game_version)
                    && (kind != ContentKind::Mod || loaders.iter().any(|l| l == loader.display_name())),
                file_id: f.id.to_string(),
                version_number: f.display_name,
                stable: f.release_type == RELEASE,
                date: f.file_date.chars().take(10).collect(),
                game_versions,
                loaders: loaders.into_iter().map(|l| l.to_lowercase()).collect(),
            }
        })
        .collect())
}

fn to_mod_file(kind: ContentKind, file: File) -> ModFile {
    ModFile {
        source: ModSource::CurseForge,
        project_id: file.mod_id.to_string(),
        file_id: file.id.to_string(),
        version_number: file.display_name,
        file_name: file.file_name,
        url: file.download_url.filter(|u| !u.is_empty()),
        size: file.file_length,
        dependencies: file
            .dependencies
            .iter()
            .filter(|d| kind == ContentKind::Mod && d.relation_type == REQUIRED)
            .map(|d| d.mod_id.to_string())
            .collect(),
    }
}

pub fn project(client: &Client, key: &str, project_id: &str) -> Result<ProjectInfo> {
    let resp: Response<Mod> = get(client, key, &format!("/mods/{project_id}"), &[])?;
    Ok(ProjectInfo { title: resp.data.name, icon_url: icon(resp.data.logo) })
}


/// A file listed in a CurseForge modpack, with what's needed to place it.
#[derive(Debug, Clone)]
pub(crate) struct PackFile {
    pub project_id: String,
    pub file_id: String,
    pub file_name: String,
    pub version_number: String,
    pub url: Option<String>,
    pub size: Option<u64>,
    pub title: String,
    pub icon_url: Option<String>,
    pub page_url: String,
    /// `None` for project types the launcher doesn't place (worlds, ...).
    pub kind: Option<ContentKind>,
}

/// Resolves the `(project id, file id)` pairs of a modpack manifest.
pub(crate) fn pack_files(client: &Client, key: &str, files: &[(u64, u64)]) -> Result<Vec<PackFile>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let file_ids: Vec<u64> = files.iter().map(|f| f.1).collect();
    let mod_ids: Vec<u64> = files.iter().map(|f| f.0).collect();
    let found: Response<Vec<File>> = post(client, key, "/mods/files", &serde_json::json!({ "fileIds": file_ids }))?;
    let mods: Response<Vec<Mod>> = post(client, key, "/mods", &serde_json::json!({ "modIds": mod_ids }))?;
    let mut mods: std::collections::HashMap<u64, Mod> = mods.data.into_iter().map(|m| (m.id, m)).collect();
    Ok(found
        .data
        .into_iter()
        .map(|f| {
            let project = mods.remove(&f.mod_id);
            let class = project.as_ref().and_then(|m| m.class_id);
            PackFile {
                project_id: f.mod_id.to_string(),
                file_id: f.id.to_string(),
                version_number: f.display_name,
                url: f.download_url.filter(|u| !u.is_empty()),
                size: f.file_length,
                // Unknown projects are most likely mods.
                kind: class.map_or(Some(ContentKind::Mod), ContentKind::from_curseforge_class),
                title: project.as_ref().map_or_else(|| f.file_name.clone(), |m| m.name.clone()),
                page_url: project.as_ref().and_then(|m| m.links.as_ref()?.website_url.clone()).unwrap_or_default(),
                icon_url: project.and_then(|m| icon(m.logo)),
                file_name: f.file_name,
            }
        })
        .collect())
}