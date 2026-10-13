//! The saved account list and which account is active.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Account, offline_uuid};
use crate::Result;
use crate::util::{now_unix, write_atomic};

/// Tokens of a Microsoft account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrosoftAuth {
    /// Microsoft refresh token; renews everything below without the user.
    pub refresh_token: String,
    /// Minecraft access token, passed to the game.
    pub access_token: String,
    /// Unix time the Minecraft token expires.
    pub expires_at: u64,
}

impl MicrosoftAuth {
    /// True when the Minecraft token is expired or about to be.
    pub fn needs_refresh(&self) -> bool {
        self.expires_at <= now_unix() + 5 * 60
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAccount {
    /// Player UUID; also identifies the account in the list.
    pub uuid: String,
    pub username: String,
    /// Present for Microsoft accounts; offline accounts have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microsoft: Option<MicrosoftAuth>,
}

impl StoredAccount {
    pub fn offline(username: &str) -> Self {
        Self { uuid: offline_uuid(username), username: username.to_string(), microsoft: None }
    }

    pub fn is_microsoft(&self) -> bool {
        self.microsoft.is_some()
    }

    /// The identity to start the game with. Microsoft tokens must be fresh already.
    pub fn launch_identity(&self) -> Account {
        match &self.microsoft {
            Some(ms) => Account {
                username: self.username.clone(),
                uuid: self.uuid.clone(),
                access_token: ms.access_token.clone(),
                user_type: "msa",
            },
            None => Account::offline(&self.username),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountStore {
    /// UUID of the account games start with.
    pub active: Option<String>,
    pub accounts: Vec<StoredAccount>,
}

impl AccountStore {
    /// Loads the list; a missing file is an empty list.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn get(&self, uuid: &str) -> Option<&StoredAccount> {
        self.accounts.iter().find(|a| a.uuid == uuid)
    }

    pub fn active(&self) -> Option<&StoredAccount> {
        self.active.as_deref().and_then(|id| self.get(id))
    }

    /// Adds the account, or replaces the one with the same UUID, and makes it active.
    pub fn upsert(&mut self, account: StoredAccount) {
        self.active = Some(account.uuid.clone());
        match self.accounts.iter_mut().find(|a| a.uuid == account.uuid) {
            Some(existing) => *existing = account,
            None => self.accounts.push(account),
        }
    }

    /// Replaces the saved copy of an account (e.g. after a token refresh) without activating it.
    pub fn replace(&mut self, account: StoredAccount) -> bool {
        match self.accounts.iter_mut().find(|a| a.uuid == account.uuid) {
            Some(existing) => {
                *existing = account;
                true
            }
            None => false,
        }
    }

    /// Removes an account; if it was active, the first remaining one takes over.
    pub fn remove(&mut self, uuid: &str) -> Option<StoredAccount> {
        let index = self.accounts.iter().position(|a| a.uuid == uuid)?;
        let removed = self.accounts.remove(index);
        if self.active.as_deref() == Some(uuid) {
            self.active = self.accounts.first().map(|a| a.uuid.clone());
        }
        Some(removed)
    }

    /// Makes an account active; false if there is no such account.
    pub fn set_active(&mut self, uuid: &str) -> bool {
        let exists = self.get(uuid).is_some();
        if exists {
            self.active = Some(uuid.to_string());
        }
        exists
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn microsoft(uuid: &str, name: &str) -> StoredAccount {
        StoredAccount {
            uuid: uuid.into(),
            username: name.into(),
            microsoft: Some(MicrosoftAuth { refresh_token: "r".into(), access_token: "a".into(), expires_at: 1 }),
        }
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::load(&dir.path().join("accounts.json")).unwrap();
        assert!(store.accounts.is_empty());
        assert!(store.active().is_none());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let mut store = AccountStore::default();
        store.upsert(StoredAccount::offline("Steve"));
        store.upsert(microsoft("abc", "Alex"));
        store.save(&path).unwrap();
        assert_eq!(AccountStore::load(&path).unwrap(), store);
    }

    #[test]
    fn upsert_replaces_and_activates() {
        let mut store = AccountStore::default();
        store.upsert(microsoft("abc", "Old"));
        store.upsert(StoredAccount::offline("Steve"));
        store.upsert(microsoft("abc", "New"));
        assert_eq!(store.accounts.len(), 2);
        assert_eq!(store.active().unwrap().username, "New");
    }

    #[test]
    fn replace_keeps_active() {
        let mut store = AccountStore::default();
        store.upsert(microsoft("abc", "Old"));
        store.upsert(StoredAccount::offline("Steve"));
        assert!(store.replace(microsoft("abc", "New")));
        assert_eq!(store.active().unwrap().username, "Steve");
        assert_eq!(store.get("abc").unwrap().username, "New");
        assert!(!store.replace(microsoft("zzz", "Nobody")));
    }

    #[test]
    fn removing_active_picks_another() {
        let mut store = AccountStore::default();
        store.upsert(StoredAccount::offline("Steve"));
        store.upsert(microsoft("abc", "Alex"));
        store.remove("abc").unwrap();
        assert_eq!(store.active().unwrap().username, "Steve");
        let steve = store.active.clone().unwrap();
        store.remove(&steve).unwrap();
        assert!(store.active.is_none());
        assert!(store.remove("missing").is_none());
    }

    #[test]
    fn switching() {
        let mut store = AccountStore::default();
        store.upsert(StoredAccount::offline("Steve"));
        store.upsert(microsoft("abc", "Alex"));
        let steve = offline_uuid("Steve");
        assert!(store.set_active(&steve));
        assert_eq!(store.active().unwrap().username, "Steve");
        assert!(!store.set_active("missing"));
        assert_eq!(store.active.as_deref(), Some(steve.as_str()));
    }

    #[test]
    fn launch_identity() {
        let offline = StoredAccount::offline("Steve").launch_identity();
        assert_eq!(offline, Account::offline("Steve"));
        let ms = microsoft("abc", "Alex").launch_identity();
        assert_eq!((ms.user_type, ms.access_token.as_str(), ms.uuid.as_str()), ("msa", "a", "abc"));
    }
}
