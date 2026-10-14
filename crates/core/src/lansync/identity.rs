//! Which launchers trust each other: those signed in with the same Microsoft account, or
//! playing under the same offline name. There is no pairing step.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncIdentity {
    key: [u8; 32],
}

impl SyncIdentity {
    /// Microsoft accounts match by player UUID, offline ones by name (ignoring case).
    pub fn new(uuid: &str, username: &str, microsoft: bool) -> Self {
        let id = if microsoft {
            format!("msa:{}", uuid.replace('-', "").to_lowercase())
        } else {
            format!("offline:{}", username.to_lowercase())
        };
        Self { key: sha256(&[b"axi-lan-sync-v1\0", id.as_bytes()]) }
    }

    /// Put in beacons so launchers of other accounts are skipped without naming the account.
    pub fn tag(&self) -> String {
        hex(&sha256(&[b"tag\0", &self.key])[..8])
    }

    /// Proves knowledge of the account to the other side of a connection.
    pub fn proof(&self, role: &str, first: &str, second: &str) -> String {
        hex(&hmac(&self.key, &[role.as_bytes(), b"\0", first.as_bytes(), b"\0", second.as_bytes()]))
    }
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// HMAC-SHA256 (RFC 2104).
fn hmac(key: &[u8; 32], parts: &[&[u8]]) -> [u8; 32] {
    let mut block = [0u8; 64];
    block[..32].copy_from_slice(key);
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let (ipad, opad) = (pad(0x36), pad(0x5c));
    let mut inner = vec![&ipad[..]];
    inner.extend_from_slice(parts);
    let inner = sha256(&inner);
    sha256(&[&opad, &inner])
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 128 unpredictable bits as hex, for nonces and ids.
pub fn random_id() -> String {
    let mut bytes = Vec::with_capacity(16);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for i in 0..2u64 {
        // Every `RandomState` is seeded from the OS's random source.
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        hasher.write_u64(i);
        bytes.extend_from_slice(&hasher.finish().to_le_bytes());
    }
    hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_account_matches() {
        let a = SyncIdentity::new("069a79f4-44e9-4726-a5be-fca90e38aaf5", "Notch", true);
        let b = SyncIdentity::new("069A79F444E94726A5BEFCA90E38AAF5", "renamed", true);
        assert_eq!(a, b);
        assert_eq!(SyncIdentity::new("x", "Steve", false), SyncIdentity::new("y", "steve", false));
        assert_ne!(SyncIdentity::new("u", "Steve", false), SyncIdentity::new("u", "Steve", true));
        assert_eq!(a.tag().len(), 16);
    }

    #[test]
    fn hmac_matches_rfc_4231_style_vector() {
        // HMAC-SHA256 with key = 32 x 0x0b and data "Hi There" (key length differs from
        // RFC 4231 case 1, so check against a value computed with a reference implementation).
        let mac = hmac(&[0x0b; 32], &[b"Hi There"]);
        assert_eq!(hex(&mac), "198a607eb44bfbc69903a0f1cf2bbdc5ba0aa3f3d9ae3c1c7a3b1696a0b68cf7");
    }

    #[test]
    fn random_ids_differ() {
        assert_ne!(random_id(), random_id());
        assert_eq!(random_id().len(), 32);
    }
}
