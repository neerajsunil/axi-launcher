mod archive;
mod detect;
mod provider;
mod runtime;
mod temurin;
mod zulu;

pub use detect::{JavaInstall, detect_installations};
pub use provider::{
    JavaProvider, JavaRelease, NEWEST_GAME_JAVA, available_releases, games_for_java, install_release, installed_release,
    installed_version, is_outdated, remove_install,
};
pub use runtime::{ensure_intel_mac_runtime, ensure_runtime};
