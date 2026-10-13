//! Mojang's `rules` blocks, which gate libraries and arguments by OS and launcher feature.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Rule {
    pub action: String,
    pub os: Option<OsRule>,
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Deserialize)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
}

/// Launcher features a rule can ask about.
#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    pub custom_resolution: bool,
}

impl Features {
    fn get(&self, name: &str) -> bool {
        match name {
            "has_custom_resolution" => self.custom_resolution,
            _ => false, // demo mode, quick play, ...
        }
    }
}

pub fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    }
}

fn os_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

fn rule_matches(rule: &Rule, features: &Features) -> bool {
    if let Some(os) = &rule.os {
        if os.name.as_deref().is_some_and(|n| n != os_name()) {
            return false;
        }
        if os.arch.as_deref().is_some_and(|a| a != os_arch()) {
            return false;
        }
    }
    rule.features
        .iter()
        .flatten()
        .all(|(name, &wanted)| features.get(name) == wanted)
}

pub fn rules_allow(rules: &[Rule], features: &Features) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules.iter().filter(|r| rule_matches(r, features)) {
        allowed = rule.action == "allow";
    }
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn rules(v: Value) -> Vec<Rule> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn empty_rules_allow() {
        assert!(rules_allow(&[], &Features::default()));
    }

    #[test]
    fn os_rules() {
        let f = Features::default();
        let only_here = rules(json!([{ "action": "allow", "os": { "name": os_name() } }]));
        assert!(rules_allow(&only_here, &f));
        let all_but_here = rules(json!([
            { "action": "allow" },
            { "action": "disallow", "os": { "name": os_name() } }
        ]));
        assert!(!rules_allow(&all_but_here, &f));
        let elsewhere = rules(json!([{ "action": "allow", "os": { "name": "nonexistent-os" } }]));
        assert!(!rules_allow(&elsewhere, &f));
    }

    #[test]
    fn feature_rules() {
        let res = rules(json!([{ "action": "allow", "features": { "has_custom_resolution": true } }]));
        assert!(!rules_allow(&res, &Features::default()));
        assert!(rules_allow(&res, &Features { custom_resolution: true }));
        let demo = rules(json!([{ "action": "allow", "features": { "is_demo_user": true } }]));
        assert!(!rules_allow(&demo, &Features { custom_resolution: true }));
    }
}
