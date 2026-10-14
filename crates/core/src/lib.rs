//! Launcher logic with no UI: Mojang metadata, downloads, instances, settings and launching.
//!
//! Only modern Minecraft (1.13+, the `arguments` launch format) is supported.

pub mod account;
pub mod built_in;
pub mod error;
pub mod instance;
pub mod java;
pub mod lansync;
pub mod launch;
pub mod loader;
pub mod meta;
pub mod modpack;
pub mod mods;
pub mod net;
pub mod paths;
pub mod settings;
pub mod update;
pub mod util;

pub use error::{Error, Result};

/// Reports a status message and a progress value in `0.0..=1.0`.
pub type Progress<'a> = &'a (dyn Fn(String, f32) + Sync);
