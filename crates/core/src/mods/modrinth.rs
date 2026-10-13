//! Modrinth API v2 (https://docs.modrinth.com). No key needed.

use std::collections::HashMap;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{ContentKind, GalleryImage, ModFile, ModHit, ModSource, ProjectDetails, ProjectInfo, ProjectVersion, SearchPage, SearchQuery, description};
use crate::Result;
use crate::loader::ModLoader;

const API: &str = "https://api.modrinth.com/v2";

fn get<T: DeserializeOwned>(client: &Client, path: &str, query: &[(&str, String)]) -> Result<T> {
    Ok(client.get(format!("{API}{path}")).query(query).send()?.error_for_status()?.json()?)
}

fn loader_id(loader: ModLoader) -> &'static str {
    match loader {
        ModLoader::Vanilla => "minecraft",
        ModLoader::Fabric => "fabric",
        ModLoader::NeoForge => "neoforge",
    }
}

/// Modrinth "loaders" a file of `kind` must have to work in the instance.
fn loaders(kind: ContentKind, loader: ModLoader) -> Vec<&'static str> {
    match kind {
        ContentKind::Mod => vec![loader_id(loader)],
        ContentKind::ResourcePack => vec!["minecraft"],
        // Iris also runs most OptiFine shaders.
        ContentKind::Shader => vec!["iris", "optifine"],
        // Packs this launcher can run.
        ContentKind::Modpack => vec!["fabric", "neoforge"],
    }
}

/// `["a","b"]` as Modrinth expects in query strings.
fn json_list(items: &[&str]) -> String {
    serde_json::to_string(items).unwrap_or_default()
}

#[derive(Deserialize)]
struct SearchResponse {
    hits: Vec<Hit>,
    total_hits: u32,
}

#[derive(Deserialize)]
struct Hit {
    project_id: String,
    slug: String,
    title: String,
    author: String,
    description: String,
    downloads: u64,
    icon_url: Option<String>,
}

pub fn search(client: &Client, q: &SearchQuery) -> Result<SearchPage> {
    // Facets are ANDed lists of ORed filters.
    let mut facets = vec![vec![format!("project_type:{}", q.kind.modrinth_type())]];
    if q.kind != ContentKind::ResourcePack {
        facets.push(loaders(q.kind, q.loader).iter().map(|l| format!("categories:{l}")).collect());
    }
    if !q.game_version.is_empty() {
        facets.push(vec![format!("versions:{}", q.game_version)]);
    }
    let facets = serde_json::to_string(&facets).unwrap_or_default();
    // Without search text, show the most popular projects.
    let index = if q.text.trim().is_empty() { "downloads" } else { "relevance" };
    let resp: SearchResponse = get(
        client,
        "/search",
        &[
            ("query", q.text.trim().to_string()),
            ("facets", facets),
            ("index", index.into()),
            ("offset", q.offset.to_string()),
            ("limit", q.limit.to_string()),
        ],
    )?;
    Ok(SearchPage {
        total: resp.total_hits,
        hits: resp
            .hits
            .into_iter()
            .map(|h| ModHit {
                source: ModSource::Modrinth,
                page_url: format!("https://modrinth.com/{}/{}", q.kind.modrinth_type(), h.slug),
                project_id: h.project_id,
                title: h.title,
                author: h.author,
                description: h.description,
                downloads: h.downloads,
                icon_url: h.icon_url.filter(|u| !u.is_empty()),
            })
            .collect(),
    })
}

#[derive(Deserialize)]
struct Version {
    id: String,
    project_id: String,
    version_number: String,
    version_type: String,
    date_published: String,
    files: Vec<File>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
}

#[derive(Deserialize)]
struct File {
    url: String,
    filename: String,
    primary: bool,
    size: u64,
}

#[derive(Deserialize)]
struct Dependency {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

pub fn latest_file(
    client: &Client,
    kind: ContentKind,
    project_id: &str,
    game_version: &str,
    loader: ModLoader,
) -> Result<Option<ModFile>> {
    let mut query = vec![("loaders", json_list(&loaders(kind, loader)))];
    if !game_version.is_empty() {
        query.push(("game_versions", json_list(&[game_version])));
    }
    let mut versions: Vec<Version> = get(client, &format!("/project/{project_id}/version"), &query)?;
    versions.sort_by(|a, b| b.date_published.cmp(&a.date_published));
    // Prefer the newest release; fall back to betas/alphas when there is none.
    let pick = versions.iter().position(|v| v.version_type == "release").or((!versions.is_empty()).then_some(0));
    let Some(version) = pick.map(|i| versions.swap_remove(i)) else { return Ok(None) };
    to_mod_file(client, kind, version)
}

/// One specific version of a project.
pub fn file(client: &Client, kind: ContentKind, version_id: &str) -> Result<Option<ModFile>> {
    let version: Version = get(client, &format!("/version/{version_id}"), &[])?;
    to_mod_file(client, kind, version)
}

/// Every version of a project, newest first, marked by whether it suits the instance.
pub fn versions(
    client: &Client,
    kind: ContentKind,
    project_id: &str,
    game_version: &str,
    loader: ModLoader,
) -> Result<Vec<ProjectVersion>> {
    let mut versions: Vec<Version> = get(client, &format!("/project/{project_id}/version"), &[])?;
    versions.sort_by(|a, b| b.date_published.cmp(&a.date_published));
    let wanted = loaders(kind, loader);
    Ok(versions
        .into_iter()
        .map(|v| ProjectVersion {
            compatible: v.game_versions.iter().any(|g| g == game_version)
                && (kind == ContentKind::ResourcePack || v.loaders.iter().any(|l| wanted.contains(&l.as_str()))),
            file_id: v.id,
            version_number: v.version_number,
            stable: v.version_type == "release",
            date: v.date_published.chars().take(10).collect(),
            game_versions: v.game_versions,
            loaders: v.loaders,
        })
        .collect())
}

fn to_mod_file(client: &Client, kind: ContentKind, version: Version) -> Result<Option<ModFile>> {
    let Some(file) = version.files.iter().find(|f| f.primary).or(version.files.first()) else { return Ok(None) };

    let mut dependencies = Vec::new();
    let required = version.dependencies.iter().filter(|d| d.dependency_type == "required");
    for dep in required.filter(|_| kind == ContentKind::Mod) {
        match (&dep.project_id, &dep.version_id) {
            (Some(project), _) => dependencies.push(project.clone()),
            (None, Some(version_id)) => {
                let v: Version = get(client, &format!("/version/{version_id}"), &[])?;
                dependencies.push(v.project_id);
            }
            _ => {}
        }
    }
    Ok(Some(ModFile {
        source: ModSource::Modrinth,
        project_id: version.project_id,
        file_id: version.id,
        version_number: version.version_number,
        file_name: file.filename.clone(),
        url: Some(file.url.clone()),
        size: Some(file.size),
        dependencies,
    }))
}

#[derive(Deserialize)]
struct Project {
    title: String,
    icon_url: Option<String>,
}

pub fn project(client: &Client, project_id: &str) -> Result<ProjectInfo> {
    let p: Project = get(client, &format!("/project/{project_id}"), &[])?;
    Ok(ProjectInfo { title: p.title, icon_url: p.icon_url.filter(|u| !u.is_empty()) })
}

#[derive(Deserialize)]
struct FullProject {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body: String,
    icon_url: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    followers: u64,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    client_side: String,
    #[serde(default)]
    server_side: String,
    license: Option<License>,
    #[serde(default)]
    published: String,
    #[serde(default)]
    updated: String,
    source_url: Option<String>,
    issues_url: Option<String>,
    wiki_url: Option<String>,
    discord_url: Option<String>,
    #[serde(default)]
    donation_urls: Vec<DonationUrl>,
    #[serde(default)]
    gallery: Vec<Gallery>,
}

#[derive(Deserialize)]
struct License {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct DonationUrl {
    url: String,
}

#[derive(Deserialize)]
struct Gallery {
    url: String,
    #[serde(default)]
    featured: bool,
    title: Option<String>,
    #[serde(default)]
    ordering: i64,
}

pub fn details(client: &Client, project_id: &str) -> Result<ProjectDetails> {
    let p: FullProject = get(client, &format!("/project/{project_id}"), &[])?;
    let mut links = Vec::new();
    for (label, url) in [("Source", p.source_url), ("Issues", p.issues_url), ("Wiki", p.wiki_url), ("Discord", p.discord_url)] {
        if let Some(url) = url.filter(|u| !u.is_empty()) {
            links.push((label.to_string(), url));
        }
    }
    if let Some(donate) = p.donation_urls.into_iter().next() {
        links.push(("Donate".into(), donate.url));
    }
    let mut gallery = p.gallery;
    // Featured images first, then the author's order.
    gallery.sort_by_key(|g| (!g.featured, g.ordering));
    let license = p.license.map(|l| if l.name.is_empty() || l.name == "Custom" { l.id } else { l.name }).unwrap_or_default();
    Ok(ProjectDetails {
        title: p.title,
        summary: p.description,
        icon_url: p.icon_url.filter(|u| !u.is_empty()),
        body: description::parse(&p.body),
        downloads: p.downloads,
        followers: p.followers,
        categories: p.categories,
        client_side: p.client_side,
        server_side: p.server_side,
        license: license.trim_start_matches("LicenseRef-").replace('-', " "),
        published: p.published.chars().take(10).collect(),
        updated: p.updated.chars().take(10).collect(),
        links,
        gallery: gallery.into_iter().map(|g| GalleryImage { url: g.url, title: g.title.unwrap_or_default() }).collect(),
    })
}

/// A published version found by one of its file hashes.
#[derive(Debug, Clone)]
pub(crate) struct HashMatch {
    pub project_id: String,
    pub version_id: String,
    pub version_number: String,
}

/// Looks up which project versions files belong to, by SHA-1. Unknown hashes are left out.
pub(crate) fn versions_by_sha1(client: &Client, hashes: &[String]) -> Result<HashMap<String, HashMatch>> {
    if hashes.is_empty() {
        return Ok(HashMap::new());
    }
    let body = serde_json::json!({ "hashes": hashes, "algorithm": "sha1" });
    let found: HashMap<String, Version> =
        client.post(format!("{API}/version_files")).json(&body).send()?.error_for_status()?.json()?;
    Ok(found
        .into_iter()
        .map(|(hash, v)| {
            (hash, HashMatch { project_id: v.project_id, version_id: v.id, version_number: v.version_number })
        })
        .collect())
}

#[derive(Deserialize)]
struct ListedProject {
    id: String,
    title: String,
    icon_url: Option<String>,
}

/// Titles and icons of many projects at once, by id.
pub(crate) fn projects(client: &Client, ids: &[String]) -> Result<HashMap<String, ProjectInfo>> {
    let mut out = HashMap::new();
    // Keeps the URL short.
    for chunk in ids.chunks(100) {
        let ids: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let list: Vec<ListedProject> = get(client, "/projects", &[("ids", json_list(&ids))])?;
        out.extend(list.into_iter().map(|p| {
            (p.id, ProjectInfo { title: p.title, icon_url: p.icon_url.filter(|u| !u.is_empty()) })
        }));
    }
    Ok(out)
}
