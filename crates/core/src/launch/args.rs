//! The `arguments.game` / `arguments.jvm` lists of a version JSON.

use std::collections::HashMap;

use serde_json::Value;

use crate::meta::rules::{Features, Rule, rules_allow};

/// Flattens an argument list, dropping entries whose rules don't apply.
pub fn collect(values: &[Value], features: &Features) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        match value {
            Value::String(s) => out.push(s.clone()),
            Value::Object(obj) => {
                let rules: Vec<Rule> = obj
                    .get("rules")
                    .and_then(|r| serde_json::from_value(r.clone()).ok())
                    .unwrap_or_default();
                if !rules_allow(&rules, features) {
                    continue;
                }
                match obj.get("value") {
                    Some(Value::String(s)) => out.push(s.clone()),
                    Some(Value::Array(items)) => {
                        out.extend(items.iter().filter_map(|v| v.as_str().map(String::from)))
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

/// Replaces `${name}` placeholders; unknown placeholders are left as-is.
pub fn substitute(arg: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&rest[start..start + 3 + end]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::rules::os_name;
    use serde_json::json;

    #[test]
    fn collects_arguments_by_rules() {
        let values = [
            json!("--username"),
            json!("${auth_player_name}"),
            json!({ "rules": [{ "action": "allow", "features": { "has_custom_resolution": true } }],
                    "value": ["--width", "${resolution_width}"] }),
            json!({ "rules": [{ "action": "allow", "os": { "name": os_name() } }],
                    "value": "-Dhere=1" }),
        ];
        assert_eq!(
            collect(&values, &Features::default()),
            ["--username", "${auth_player_name}", "-Dhere=1"]
        );
        assert_eq!(
            collect(&values, &Features { custom_resolution: true }),
            ["--username", "${auth_player_name}", "--width", "${resolution_width}", "-Dhere=1"]
        );
    }

    #[test]
    fn substitutes_placeholders() {
        let vars = HashMap::from([("a", "1".to_string()), ("dir", "C:/x".to_string())]);
        assert_eq!(substitute("-Dp=${dir}/lib", &vars), "-Dp=C:/x/lib");
        assert_eq!(substitute("${a}${a}", &vars), "11");
        assert_eq!(substitute("${unknown} ${a}", &vars), "${unknown} 1");
        assert_eq!(substitute("broken ${a", &vars), "broken ${a");
    }
}
