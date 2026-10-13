//! Turning an instance into a running game.

mod args;
mod natives;
mod prepare;
mod process;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use reqwest::blocking::Client;

pub use process::{GameProcess, OutputSink, run_logged};

use crate::account::Account;
use crate::instance::{EffectiveSettings, Instance};
use crate::loader;
use crate::meta::VersionEntry;
use crate::meta::rules::Features;
use crate::paths::Paths;
use crate::settings::GlobalSettings;
use crate::{Error, Progress, Result};

pub struct LaunchContext<'a> {
    pub paths: &'a Paths,
    pub client: &'a Client,
    /// Known versions, used to download version JSONs that are not cached yet.
    pub versions: &'a [VersionEntry],
}

/// Downloads everything the instance needs and starts the game.
pub fn launch(
    ctx: &LaunchContext,
    instance: &Instance,
    settings: &GlobalSettings,
    account: &Account,
    progress: Progress,
    output: OutputSink,
) -> Result<Arc<GameProcess>> {
    let config = &instance.config;
    let effective = EffectiveSettings::resolve(settings, config);
    let mut loader_version = config.loader_version.clone();
    if config.loader.is_modded() && loader_version.is_empty() {
        let versions = loader::fetch_versions(ctx.client, config.loader, &config.game_version)?;
        loader_version = loader::recommended(&versions)
            .ok_or_else(|| {
                Error::msg(format!("{} is not available for {}", config.loader.display_name(), config.game_version))
            })?
            .version
            .clone();
    }
    let target = prepare::Target {
        game_version: &config.game_version,
        loader: config.loader,
        loader_version: &loader_version,
    };
    let prepared = prepare::prepare(
        ctx,
        &target,
        &effective.java_path,
        settings.concurrent_downloads,
        progress,
        &output,
    )?;
    let version = &prepared.version;
    let game_dir = instance.game_dir();
    fs::create_dir_all(&game_dir)?;

    let window = effective.window;
    let features = Features { custom_resolution: !window.fullscreen };
    let arguments = version.arguments.as_ref().ok_or_else(|| Error::msg("missing arguments"))?;

    let sep = if cfg!(windows) { ";" } else { ":" };
    let classpath = prepared
        .classpath
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join(sep);
    let path_str = |p: &Path| p.to_string_lossy().into_owned();
    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", account.username.clone()),
        ("auth_uuid", account.uuid.clone()),
        ("auth_access_token", account.access_token.clone()),
        ("auth_xuid", "0".into()),
        ("clientid", "0".into()),
        ("user_type", account.user_type.into()),
        ("version_name", version.profile_id.clone().unwrap_or_else(|| version.id.clone())),
        ("version_type", version.kind.clone()),
        ("game_directory", path_str(&game_dir)),
        ("assets_root", path_str(&ctx.paths.assets_dir())),
        ("assets_index_name", version.asset_index.id.clone()),
        ("natives_directory", path_str(&prepared.natives_dir)),
        ("library_directory", path_str(&ctx.paths.libraries_dir())),
        ("classpath_separator", sep.into()),
        ("classpath", classpath),
        ("resolution_width", window.width.to_string()),
        ("resolution_height", window.height.to_string()),
        ("launcher_name", "axi-launcher".into()),
        ("launcher_version", env!("CARGO_PKG_VERSION").into()),
    ]);

    let mut jvm_args = vec![
        format!("-Xms{}M", effective.min_memory_mb),
        format!("-Xmx{}M", effective.max_memory_mb),
    ];
    jvm_args.extend(effective.jvm_args.iter().cloned());
    jvm_args.extend(args::collect(&arguments.jvm, &features).iter().map(|a| args::substitute(a, &vars)));
    let mut game_args: Vec<String> =
        args::collect(&arguments.game, &features).iter().map(|a| args::substitute(a, &vars)).collect();
    if window.fullscreen {
        game_args.push("--fullscreen".into());
    }

    output(format!(
        "[launcher] Starting {} with {}\n[launcher] JVM args: {}",
        version.profile_id.as_deref().unwrap_or(&version.id),
        prepared.java.display(),
        jvm_args
            .iter()
            .map(|a| if a.len() > 300 && a.contains(sep) { "<classpath>" } else { a.as_str() })
            .collect::<Vec<_>>()
            .join(" ")
    ));
    progress(format!("Starting Minecraft {}...", version.id), 1.0);

    let mut cmd = Command::new(&prepared.java);
    cmd.args(&jvm_args)
        .arg(&version.main_class)
        .args(&game_args)
        .current_dir(&game_dir);
    GameProcess::spawn(cmd, &instance.log_file(), output)
        .map_err(|e| Error::msg(format!("starting {}: {e}", prepared.java.display())))
}
