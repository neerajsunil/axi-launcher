//! Player accounts: offline names and Microsoft accounts, stored in `accounts.json`.

pub mod microsoft;
mod offline;
pub mod skin;
mod store;

pub use offline::offline_uuid;
pub use store::{AccountStore, MicrosoftAuth, StoredAccount};

/// The identity a game is started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub user_type: &'static str,
}

impl Account {
    pub fn offline(username: &str) -> Self {
        Self {
            username: username.to_string(),
            uuid: offline_uuid(username),
            access_token: "0".into(),
            user_type: "legacy",
        }
    }
}
