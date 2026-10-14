//! Keys compiled in from the `AXI_CURSEFORGE_API_KEY` and `AXI_MSA_CLIENT_ID` environment
//! variables (repository secrets in CI). They're stored scrambled (see `build.rs`) so they
//! don't show up as plain text in the binary. Empty when the build had none.

mod keys {
    include!(concat!(env!("OUT_DIR"), "/built_in_keys.rs"));
}

fn reveal((data, mask): (&[u8], &[u8])) -> String {
    // Keeps the compiler from unscrambling it at build time.
    let mask = std::hint::black_box(mask);
    data.iter().zip(mask).map(|(d, m)| char::from(d ^ m)).collect()
}

/// The CurseForge API key this launcher was built with.
pub fn curseforge_key() -> String {
    reveal(keys::CURSEFORGE_KEY)
}

/// The Azure client ID this launcher was built with.
pub fn msa_client_id() -> String {
    reveal(keys::MSA_CLIENT_ID)
}

#[cfg(test)]
mod tests {
    #[test]
    fn unscrambles_to_the_original() {
        assert_eq!(super::curseforge_key(), option_env!("AXI_CURSEFORGE_API_KEY").unwrap_or_default().trim());
        assert_eq!(super::msa_client_id(), option_env!("AXI_MSA_CLIENT_ID").unwrap_or_default().trim());
    }
}
