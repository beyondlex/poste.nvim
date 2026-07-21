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

/// Substitute {{var}} references in a string using the provided variables.
pub fn substitute_vars(input: &str, vars: &std::collections::HashMap<String, String>) -> String {
    let re = regex::Regex::new(r"\{\{([^}]+)\}\}").unwrap();
    re.replace_all(input, |caps: &regex::Captures| {
        let var_name = &caps[1];
        vars.get(var_name)
            .cloned()
            .unwrap_or_else(|| caps[0].to_string())
    })
    .to_string()
}
