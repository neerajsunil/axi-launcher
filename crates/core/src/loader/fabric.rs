//! Fabric, via meta.fabricmc.net. Its profile JSON lists Maven libraries; nothing to run.

use std::fs;

use reqwest::blocking::Client;
use serde::Deserialize;

use super::{InstallContext, LoaderVersion};
use crate::Result;
use crate::meta::version::LoaderProfile;
use crate::net::{download, get_json};

const META: &str = "https://meta.fabricmc.net/v2/versions/loader";

#[derive(Deserialize)]
struct Entry {
    loader: Loader,
}

#[derive(Deserialize)]
struct Loader {
    version: String,
    stable: bool,
}

pub fn versions(client: &Client, game_version: &str) -> Result<Vec<LoaderVersion>> {
    let entries: Vec<Entry> = get_json(client, &format!("{META}/{}", encode(game_version)))?;
    Ok(entries
        .into_iter()
        .map(|e| LoaderVersion { version: e.loader.version, stable: e.loader.stable })
        .collect())
}

pub fn install(ctx: &InstallContext, game_version: &str, version: &str) -> Result<LoaderProfile> {
    let id = format!("fabric-loader-{version}-{game_version}");
    let path = ctx.paths.version_dir(&id).join(format!("{id}.json"));
    if !path.exists() {
        (ctx.progress)(format!("Fetching Fabric {version}..."), 0.0);
        let url = format!("{META}/{}/{}/profile/json", encode(game_version), encode(version));
        download(ctx.client, &url, &path, None)?;
    }
    Ok(serde_json::from_slice(&fs::read(&path)?)?)
}

fn encode(segment: &str) -> String {
    segment.replace(' ', "%20").replace('+', "%2B")
}
