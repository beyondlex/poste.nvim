use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    #[serde(flatten)]
    pub envs: HashMap<String, HashMap<String, String>>,
}

impl Environment {
    pub fn parse(content: &str) -> Result<Self> {
        let envs: HashMap<String, HashMap<String, String>> = serde_json::from_str(content)?;
        Ok(Self { envs })
    }

    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    pub fn get(&self, env_name: &str, var_name: &str) -> Option<&String> {
        self.envs.get(env_name)?.get(var_name)
    }
}

/// Substitute `{{var}}` references in a string using the provided variables.
///
/// A value may itself contain references (`{"dev": {"HOST": "{{ENV}}-db",
/// "ENV": "prod"}}`), so this resolves to a fixed point rather than making one
/// pass — a single pass handed the CLI a host spelled `{{ENV}}-db` while the
/// editor, whose `substitute_vars` recurses, connected happily.  The round cap
/// is the same 10 the Lua copy uses, which is what stops a self-referencing
/// value (`A={{A}}`) from looping.
pub fn substitute_vars(input: &str, vars: &std::collections::HashMap<String, String>) -> String {
    let re = regex::Regex::new(r"\{\{([^}]+)\}\}").unwrap();
    let mut result = input.to_string();
    for _ in 0..10 {
        let next = re.replace_all(&result, |caps: &regex::Captures| {
            let var_name = &caps[1];
            vars.get(var_name)
                .cloned()
                .unwrap_or_else(|| caps[0].to_string())
        });
        if next == result {
            break;
        }
        result = next.into_owned();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn test_substitute_resolves_known_and_keeps_unknown_literal() {
        let v = vars(&[("HOST", "db.example.com")]);
        assert_eq!(
            substitute_vars("postgres://{{HOST}}:5432", &v),
            "postgres://db.example.com:5432"
        );
        assert_eq!(
            substitute_vars("{{MISSING}}", &v),
            "{{MISSING}}",
            "an unknown reference must stay literal, not become empty"
        );
    }

    #[test]
    fn test_substitute_resolves_a_reference_inside_a_value() {
        let v = vars(&[("HOST", "{{ENV}}-db.example.com"), ("ENV", "prod")]);
        assert_eq!(
            substitute_vars("h={{HOST}}", &v),
            "h=prod-db.example.com",
            "one pass left the inner reference in place"
        );
    }

    /// A cycle must return something instead of hanging.  `{{A}}` ⇄ `{{B}}`
    /// never converges, so it is the cap on rounds that ends the loop — the
    /// answer is one of the two references, whichever side the last round
    /// landed on.
    #[test]
    fn test_substitute_stops_at_the_round_cap_on_a_cycle() {
        let v = vars(&[("A", "{{B}}"), ("B", "{{A}}")]);
        let out = substitute_vars("{{A}}", &v);
        assert!(
            out == "{{A}}" || out == "{{B}}",
            "a cycle must stay unresolved, not expand or vanish: {out}"
        );

        let v = vars(&[("A", "{{A}}")]);
        assert_eq!(substitute_vars("x={{A}}", &v), "x={{A}}");
    }
}
