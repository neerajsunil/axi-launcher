//! Mojang's public game metadata.

pub mod assets;
pub mod manifest;
pub mod rules;
pub mod version;

pub use manifest::{VersionEntry, fetch_versions};
pub use version::VersionJson;
