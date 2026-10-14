//! A single version's JSON: main class, libraries, arguments, assets and Java requirement.

use std::collections::HashMap;
use std::fs;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;

use super::manifest::VersionEntry;
use super::rules::Rule;
use crate::paths::Paths;
use crate::{Error, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    /// Id of the mod loader profile merged into this version, if any.
    #[serde(skip)]
    pub profile_id: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub main_class: String,
    pub asset_index: AssetIndexRef,
    pub downloads: VersionDownloads,
    #[serde(default)]
    pub libraries: Vec<Library>,
    pub arguments: Option<Arguments>,
    pub java_version: Option<JavaVersion>,
}

#[derive(Debug, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Deserialize)]
pub struct VersionDownloads {
    pub client: Artifact,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Artifact {
    pub path: Option<String>,
    pub url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Deserialize)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibraryDownloads>,
    /// Maven repository for libraries listed only by name (Fabric).
    pub url: Option<String>,
    pub size: Option<u64>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Pre-1.19 only: OS name -> classifier of a jar with native libraries to extract.
    pub natives: Option<HashMap<String, String>>,
    pub extract: Option<Extract>,
}

impl Library {
    /// The jar to put on the classpath, from `downloads` or derived from the Maven name.
    pub fn artifact(&self) -> Option<Artifact> {
        if let Some(artifact) = self.downloads.as_ref().and_then(|d| d.artifact.clone()) {
            return Some(artifact);
        }
        let repo = self.url.as_ref()?;
        let path = maven_path(&self.name)?;
        Some(Artifact {
            url: format!("{}/{path}", repo.trim_end_matches('/')),
            path: Some(path),
            size: self.size.unwrap_or(0),
        })
    }

    /// `group:artifact[:classifier]`, used to let a loader replace vanilla libraries.
    fn key(&self) -> String {
        let parts: Vec<&str> = self.name.split('@').next().unwrap_or("").split(':').collect();
        match parts.as_slice() {
            [group, artifact, _, classifier, ..] => format!("{group}:{artifact}:{classifier}"),
            [group, artifact, ..] => format!("{group}:{artifact}"),
            _ => self.name.clone(),
        }
    }
}

/// Repository path of a Maven coordinate `group:artifact:version[:classifier][@ext]`.
pub fn maven_path(name: &str) -> Option<String> {
    let (coords, ext) = name.split_once('@').unwrap_or((name, "jar"));
    let parts: Vec<&str> = coords.split(':').collect();
    let (group, artifact, version) = (parts.first()?, parts.get(1)?, parts.get(2)?);
    let classifier = parts.get(3).map(|c| format!("-{c}")).unwrap_or_default();
    Some(format!(
        "{}/{artifact}/{version}/{artifact}-{version}{classifier}.{ext}",
        group.replace('.', "/")
    ))
}

#[derive(Debug, Deserialize)]
pub struct LibraryDownloads {
    pub artifact: Option<Artifact>,
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Value>,
    #[serde(default)]
    pub jvm: Vec<Value>,
}

/// A mod loader's version JSON (`inheritsFrom` a vanilla version).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderProfile {
    pub id: String,
    pub main_class: String,
    #[serde(default)]
    pub libraries: Vec<Library>,
    pub arguments: Option<Arguments>,
}

impl VersionJson {
    /// Layers a loader profile on top: its main class and arguments win, and its
    /// libraries come first and replace vanilla ones with the same group and name.
    pub fn apply_loader(&mut self, profile: LoaderProfile) {
        let keys: std::collections::HashSet<String> = profile.libraries.iter().map(Library::key).collect();
        let vanilla = std::mem::take(&mut self.libraries);
        self.libraries = profile.libraries;
        self.libraries.extend(vanilla.into_iter().filter(|l| !keys.contains(&l.key())));
        self.main_class = profile.main_class;
        if let (Some(base), Some(extra)) = (self.arguments.as_mut(), profile.arguments) {
            base.jvm.extend(extra.jvm);
            base.game.extend(extra.game);
        }
        self.profile_id = Some(profile.id);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
    #[serde(default)]
    pub major_version: u32,
}

/// Loads the JSON for version `id`, downloading it once from the manifest entry if needed.
pub fn load_version_json(
    client: &Client,
    paths: &Paths,
    id: &str,
    entry: Option<&VersionEntry>,
) -> Result<VersionJson> {
    let path = paths.version_dir(id).join(format!("{id}.json"));
    if !path.exists() {
        let entry = entry.ok_or_else(|| Error::msg(format!("Unknown game version {id}")))?;
        crate::net::download(client, &entry.url, &path, None)?;
    }
    let json: VersionJson = serde_json::from_slice(&fs::read(&path)?)?;
    if json.arguments.is_none() {
        return Err(Error::msg(format!("{} is too old. Axi supports 1.13 and newer.", json.id)));
    }
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_paths() {
        assert_eq!(
            maven_path("net.fabricmc:fabric-loader:0.17.3").unwrap(),
            "net/fabricmc/fabric-loader/0.17.3/fabric-loader-0.17.3.jar"
        );
        assert_eq!(
            maven_path("net.minecraft:client:1.21.10:mappings@txt").unwrap(),
            "net/minecraft/client/1.21.10/client-1.21.10-mappings.txt"
        );
        assert!(maven_path("broken").is_none());
    }

    #[test]
    fn loader_replaces_matching_libraries() {
        let lib = |name: &str| Library {
            name: name.into(),
            downloads: None,
            url: Some("https://maven.example/".into()),
            size: None,
            rules: Vec::new(),
            natives: None,
            extract: None,
        };
        let mut version: VersionJson = serde_json::from_str(
            r#"{"id":"1.21.10","type":"release","mainClass":"net.minecraft.client.main.Main",
                "assetIndex":{"id":"27","url":"","size":0},
                "downloads":{"client":{"url":"","size":0}},
                "arguments":{"game":["--a"],"jvm":["-Dx"]}}"#,
        )
        .unwrap();
        version.libraries = vec![lib("org.ow2.asm:asm:9.6"), lib("org.lwjgl:lwjgl:3.3.3"), lib("org.lwjgl:lwjgl:3.3.3:natives-windows")];
        version.apply_loader(LoaderProfile {
            id: "fabric-loader-0.17.3-1.21.10".into(),
            main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".into(),
            libraries: vec![lib("org.ow2.asm:asm:9.9"), lib("net.fabricmc:fabric-loader:0.17.3")],
            arguments: Some(Arguments { game: vec![], jvm: vec!["-DFabricMcEmu".into()] }),
        });
        let names: Vec<&str> = version.libraries.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            ["org.ow2.asm:asm:9.9", "net.fabricmc:fabric-loader:0.17.3", "org.lwjgl:lwjgl:3.3.3", "org.lwjgl:lwjgl:3.3.3:natives-windows"]
        );
        assert_eq!(version.main_class, "net.fabricmc.loader.impl.launch.knot.KnotClient");
        assert_eq!(version.arguments.as_ref().unwrap().jvm.len(), 2);
        assert_eq!(version.profile_id.as_deref(), Some("fabric-loader-0.17.3-1.21.10"));
        assert_eq!(
            version.libraries[1].artifact().unwrap().url,
            "https://maven.example/net/fabricmc/fabric-loader/0.17.3/fabric-loader-0.17.3.jar"
        );
    }
}
