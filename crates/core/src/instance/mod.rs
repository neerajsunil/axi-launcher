//! Instances: self-contained game setups, each with its own `.minecraft` folder.

mod effective;
mod manager;
mod model;

pub use effective::EffectiveSettings;
pub use manager::{GameSetup, InstanceManager, unique_name};
pub use model::{Instance, InstanceConfig, MemoryOverride, Overrides, PackOrigin};
