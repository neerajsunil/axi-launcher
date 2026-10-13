//! Offline accounts: any name, a UUID derived from it, no authentication.

/// Stable per-username UUID so offline worlds keep the same player data.
pub fn offline_uuid(username: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let half = |salt: u8| {
        let mut h = DefaultHasher::new();
        ("OfflinePlayer:", username, salt).hash(&mut h);
        h.finish()
    };
    let (a, b) = (half(0), half(1));
    let a = (a & !0xF000) | 0x3000; // version 3
    let b = (b & !(0xC000u64 << 48)) | (0x8000u64 << 48); // RFC 4122 variant
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xFFFF,
        a & 0xFFFF,
        b >> 48,
        b & 0xFFFF_FFFF_FFFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_is_stable_and_well_formed() {
        let id = offline_uuid("Steve");
        assert_eq!(id, offline_uuid("Steve"));
        assert_ne!(id, offline_uuid("Alex"));
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "3");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }
}
