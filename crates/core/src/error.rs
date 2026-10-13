use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid config file: {0}")]
    TomlRead(#[from] toml::de::Error),
    #[error("could not write config: {0}")]
    TomlWrite(#[from] toml::ser::Error),
    #[error("invalid archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("{0}")]
    Msg(String),
}

impl Error {
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Msg(message.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
