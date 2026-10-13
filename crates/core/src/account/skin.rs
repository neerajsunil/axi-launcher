//! Player skins from Mojang's session server (public, no sign-in needed).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::Result;

const PROFILE_URL: &str = "https://sessionserver.mojang.com/session/minecraft/profile";

#[derive(Deserialize)]
struct Profile {
    #[serde(default)]
    properties: Vec<Property>,
}

#[derive(Deserialize)]
struct Property {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct Textures {
    textures: TextureSet,
}

#[derive(Deserialize)]
struct TextureSet {
    #[serde(rename = "SKIN")]
    skin: Option<Texture>,
}

#[derive(Deserialize)]
struct Texture {
    url: String,
}

/// The skin texture URL of the player with this UUID, or `None` when they use a default skin.
pub fn skin_url(client: &Client, uuid: &str) -> Result<Option<String>> {
    let id = uuid.replace('-', "");
    let profile: Profile = client.get(format!("{PROFILE_URL}/{id}")).send()?.error_for_status()?.json()?;
    let Some(textures) = profile.properties.into_iter().find(|p| p.name == "textures") else {
        return Ok(None);
    };
    Ok(parse_textures(&textures.value)?.map(|url| url.replacen("http://", "https://", 1)))
}

/// Reads the skin URL out of the base64 `textures` property.
fn parse_textures(value: &str) -> Result<Option<String>> {
    let json = STANDARD.decode(value).map_err(|e| crate::Error::msg(format!("invalid textures property: {e}")))?;
    let textures: Textures = serde_json::from_slice(&json)?;
    Ok(textures.textures.skin.map(|s| s.url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_skin_url_from_textures() {
        let json = r#"{"timestamp":1,"profileName":"x","textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/abc"}}}"#;
        let value = STANDARD.encode(json);
        assert_eq!(parse_textures(&value).unwrap().as_deref(), Some("http://textures.minecraft.net/texture/abc"));
        assert_eq!(parse_textures(&STANDARD.encode(r#"{"textures":{}}"#)).unwrap(), None);
    }
}
