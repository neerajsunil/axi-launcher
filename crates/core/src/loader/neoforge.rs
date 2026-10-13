//! NeoForge (1.20.2+). Installing runs NeoForge's own installer headlessly, which
//! patches the game jar and writes `versions/neoforge-<v>/` into our data directory
//! (it uses the same layout as `.minecraft`).

use std::cmp::Ordering;
use std::fs;
use std::process::Command;

use reqwest::blocking::Client;
use serde::Deserialize;

use super::{InstallContext, LoaderVersion};
use crate::launch::run_logged;
use crate::meta::version::LoaderProfile;
use crate::net::{download, get_json};
use crate::util::write_atomic;
use crate::{Error, Result};

const MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
const VERSIONS: &str = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";

#[derive(Deserialize)]
struct VersionList {
    versions: Vec<String>,
}

pub fn versions(client: &Client, game_version: &str) -> Result<Vec<LoaderVersion>> {
    let Some(prefix) = version_prefix(game_version) else { return Ok(Vec::new()) };
    let list: VersionList = get_json(client, VERSIONS)?;
    let mut versions: Vec<LoaderVersion> = list
        .versions
        .into_iter()
        .filter(|v| v.starts_with(&prefix) && !v.contains('+'))
        .map(|v| LoaderVersion { stable: !v.contains('-'), version: v })
        .collect();
    versions.sort_by(|a, b| compare(&b.version, &a.version));
    Ok(versions)
}

/// NeoForge versions start with the game version minus its leading "1.":
/// 1.21.10 -> `21.10.`, 1.21 -> `21.0.`; year-based releases: 26.1 -> `26.1.0.`.
fn version_prefix(game_version: &str) -> Option<String> {
    if !game_version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None; // snapshots and pre-releases
    }
    let parts: Vec<&str> = game_version.split('.').collect();
    match parts.as_slice() {
        ["1", minor, rest @ ..] if rest.len() <= 1 => Some(format!("{minor}.{}.", rest.first().unwrap_or(&"0"))),
        [year, drop, rest @ ..] if *year != "1" && rest.len() <= 1 => {
            Some(format!("{year}.{drop}.{}.", rest.first().unwrap_or(&"0")))
        }
        _ => None,
    }
}

/// Compares versions numerically; a stable release sorts above betas of the same number.
fn compare(a: &str, b: &str) -> Ordering {
    let numbers = |v: &str| -> Vec<u32> {
        v.split('-').next().unwrap_or("").split('.').map(|n| n.parse().unwrap_or(0)).collect()
    };
    numbers(a).cmp(&numbers(b)).then_with(|| (!a.contains('-')).cmp(&!b.contains('-')))
}

pub fn install(ctx: &InstallContext, game_version: &str, version: &str) -> Result<LoaderProfile> {
    let id = format!("neoforge-{version}");
    let dir = ctx.paths.version_dir(&id);
    let json = dir.join(format!("{id}.json"));
    let marker = dir.join(".installed");
    if !(json.exists() && marker.exists()) {
        run_installer(ctx, game_version, version)?;
        if !json.exists() {
            return Err(Error::msg("The NeoForge installer finished but did not create its version file"));
        }
        fs::File::create(&marker)?;
    }
    Ok(serde_json::from_slice(&fs::read(&json)?)?)
}

fn run_installer(ctx: &InstallContext, game_version: &str, version: &str) -> Result<()> {
    (ctx.progress)(format!("Downloading NeoForge {version}..."), 0.0);
    let installers = ctx.paths.cache_dir().join("installers");
    let installer = installers.join(format!("neoforge-{version}-installer.jar"));
    download(ctx.client, &format!("{MAVEN}/{version}/neoforge-{version}-installer.jar"), &installer, None)?;

    // The installer refuses to run without a launcher profile file in the target directory.
    let profiles = ctx.paths.root().join("launcher_profiles.json");
    if !profiles.exists() {
        write_atomic(&profiles, r#"{"profiles":{}}"#)?;
    }

    (ctx.progress)(format!("Installing NeoForge {version} for {game_version}..."), 1.0);
    (ctx.output)(format!("[launcher] Running the NeoForge {version} installer"));
    let mut cmd = Command::new(ctx.java);
    cmd.arg("-jar").arg(&installer).arg("--install-client").arg(ctx.paths.root()).current_dir(&installers);
    let status = run_logged(cmd, ctx.output)
        .map_err(|e| Error::msg(format!("could not run the NeoForge installer: {e}")))?;
    let _ = fs::remove_file(&installer);
    let _ = fs::remove_file(installers.join(format!("neoforge-{version}-installer.jar.log")));
    if !status.success() {
        return Err(Error::msg(format!("The NeoForge installer failed ({status}). See the Logs tab.")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        assert_eq!(version_prefix("1.21.10").as_deref(), Some("21.10."));
        assert_eq!(version_prefix("1.21").as_deref(), Some("21.0."));
        assert_eq!(version_prefix("26.1").as_deref(), Some("26.1.0."));
        assert_eq!(version_prefix("26.1.2").as_deref(), Some("26.1.2."));
        assert_eq!(version_prefix("26.4-snapshot-3"), None);
        assert_eq!(version_prefix("1.21.10-rc1"), None);
    }

    #[test]
    fn ordering() {
        let mut v = vec!["21.10.9-beta", "21.10.64", "21.10.10", "21.10.64-beta"];
        v.sort_by(|a, b| compare(b, a));
        assert_eq!(v, ["21.10.64", "21.10.64-beta", "21.10.10", "21.10.9-beta"]);
    }
}
