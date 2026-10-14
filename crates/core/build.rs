//! Scrambles the built-in keys so they don't sit in the binary as plain text.

use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, path::Path};

const KEYS: [(&str, &str); 2] = [("CURSEFORGE_KEY", "AXI_CURSEFORGE_API_KEY"), ("MSA_CLIENT_ID", "AXI_MSA_CLIENT_ID")];

fn main() {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut seed = nanos ^ (u64::from(std::process::id()) << 32) | 1;
    let mut code = String::new();
    for (name, var) in KEYS {
        println!("cargo:rerun-if-env-changed={var}");
        let value = env::var(var).unwrap_or_default();
        let (mut data, mut mask) = (Vec::new(), Vec::new());
        for byte in value.trim().bytes() {
            // xorshift64
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let m = (seed >> 24) as u8;
            data.push(byte ^ m);
            mask.push(m);
        }
        code += &format!("pub const {name}: (&[u8], &[u8]) = (&{data:?}, &{mask:?});\n");
    }
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("built_in_keys.rs");
    fs::write(out, code).unwrap();
}
