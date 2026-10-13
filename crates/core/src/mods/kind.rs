use serde::{Deserialize, Serialize};

/// What sort of project something on Modrinth or CurseForge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    #[default]
    Mod,
    ResourcePack,
    Shader,
    /// A whole instance; installed with `modpack`, not into an existing instance.
    Modpack,
}

impl ContentKind {
    /// The kinds that live inside an instance, in the order the UI lists them.
    pub const INSTANCE: [ContentKind; 3] = [ContentKind::Mod, ContentKind::ResourcePack, ContentKind::Shader];

    /// Folder inside `.minecraft`.
    pub fn folder(self) -> &'static str {
        match self {
            ContentKind::Mod => "mods",
            ContentKind::ResourcePack => "resourcepacks",
            ContentKind::Shader => "shaderpacks",
            ContentKind::Modpack => "modpacks",
        }
    }

    /// Extension of the files the game loads from the folder.
    pub fn extension(self) -> &'static str {
        match self {
            ContentKind::Mod => ".jar",
            _ => ".zip",
        }
    }

    /// Lowercase plural for messages ("resource packs").
    pub fn plural(self) -> &'static str {
        match self {
            ContentKind::Mod => "mods",
            ContentKind::ResourcePack => "resource packs",
            ContentKind::Shader => "shaders",
            ContentKind::Modpack => "modpacks",
        }
    }

    /// Whether the instance needs a mod loader for this to work.
    pub fn needs_loader(self) -> bool {
        matches!(self, ContentKind::Mod | ContentKind::Shader)
    }

    pub(crate) fn modrinth_type(self) -> &'static str {
        match self {
            ContentKind::Mod => "mod",
            ContentKind::ResourcePack => "resourcepack",
            ContentKind::Shader => "shader",
            ContentKind::Modpack => "modpack",
        }
    }

    pub(crate) fn curseforge_class(self) -> u32 {
        match self {
            ContentKind::Mod => 6,
            ContentKind::ResourcePack => 12,
            ContentKind::Shader => 6552,
            ContentKind::Modpack => 4471,
        }
    }

    pub(crate) fn from_curseforge_class(class: u32) -> Option<Self> {
        [ContentKind::Mod, ContentKind::ResourcePack, ContentKind::Shader, ContentKind::Modpack]
            .into_iter()
            .find(|k| k.curseforge_class() == class)
    }

    /// The kind whose folder a pack-relative path like `mods/x.jar` is in.
    pub(crate) fn from_path(path: &str) -> Option<Self> {
        let folder = path.split('/').next()?;
        ContentKind::INSTANCE.into_iter().find(|k| k.folder() == folder)
    }
}

/// Projects that let Minecraft load shader packs.
pub(crate) const SHADER_LOADERS: [&str; 4] = [
    "YL57xq9U", // Iris, Modrinth
    "GchcoXML", // Oculus, Modrinth
    "455508",   // Iris, CurseForge
    "581495",   // Oculus, CurseForge
];

/// Iris on Modrinth, which the UI offers to install for shaders.
pub const IRIS_MODRINTH_ID: &str = "YL57xq9U";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(ContentKind::from_path("shaderpacks/bsl.zip"), Some(ContentKind::Shader));
        assert_eq!(ContentKind::from_path("config/x.toml"), None);
        assert_eq!(ContentKind::from_curseforge_class(12), Some(ContentKind::ResourcePack));
        assert_eq!(ContentKind::from_curseforge_class(17), None);
    }
}
