//! Variable resolver with a clearly defined priority chain.
//!
//! Priority (higher wins):
//!   1. import_params  — caller-specified overrides (e.g. `run #Login (@env=staging)`)
//!   2. request_vars   — `@var` definitions inside a request block + pre-script injected vars
//!   3. file_vars      — `@var` definitions in file header (before the first `###`)
//!   4. session_vars   — `client.global` variables (from Lua → CLI)
//!   5. script_vars    — `script_variables` table (from Lua → CLI)
//!   6. env            — `env.json` environment variables
//!   7. magic          — built-in functions ($timestamp, $uuid, $date, $randomInt)

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct VarResolver {
    import_params: HashMap<String, String>,
    request_vars: HashMap<String, String>,
    file_vars: HashMap<String, String>,
    session_vars: HashMap<String, String>,
    script_vars: HashMap<String, String>,
    env: HashMap<String, String>,
}

impl Default for VarResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl VarResolver {
    /// Create an empty resolver.
    pub fn new() -> Self {
        Self {
            import_params: HashMap::new(),
            request_vars: HashMap::new(),
            file_vars: HashMap::new(),
            session_vars: HashMap::new(),
            script_vars: HashMap::new(),
            env: HashMap::new(),
        }
    }

    // -- Builder-style setters --

    pub fn with_import_params(mut self, vars: HashMap<String, String>) -> Self {
        self.import_params = vars;
        self
    }

    pub fn with_request_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.request_vars = vars;
        self
    }

    pub fn with_file_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.file_vars = vars;
        self
    }

    pub fn with_session_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.session_vars = vars;
        self
    }

    pub fn with_script_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.script_vars = vars;
        self
    }

    pub fn with_env(mut self, vars: HashMap<String, String>) -> Self {
        self.env = vars;
        self
    }

    /// Resolve a single variable by name, following the priority chain.
    /// Returns `None` if the variable is not found in any layer (including magic).
    pub fn resolve(&self, name: &str) -> Option<String> {
        if let Some(val) = self
            .import_params
            .get(name)
            .or_else(|| self.request_vars.get(name))
            .or_else(|| self.file_vars.get(name))
            .or_else(|| self.session_vars.get(name))
            .or_else(|| self.script_vars.get(name))
            .or_else(|| self.env.get(name))
        {
            return Some(val.clone());
        }
        Self::resolve_magic_var(name)
    }

    /// Resolve all `{{var}}` placeholders in the input string using the priority chain.
    /// Iteratively resolves: if `{{token}}` → `{{admin_token}}`, another pass resolves the inner ref.
    /// Caps at 20 iterations to prevent infinite loops from circular references.
    pub fn substitute(&self, input: &str) -> String {
        static VAR_RE: OnceLock<Regex> = OnceLock::new();
        let re = VAR_RE.get_or_init(|| {
            Regex::new(r"\{\{([^}]+)\}\}").expect("valid literal regex: {{var}}")
        });
        let mut result = input.to_string();
        for _ in 0..20 {
            let next = re
                .replace_all(&result, |caps: &regex::Captures| {
                    let var_name = &caps[1];
                    self.resolve(var_name)
                        .unwrap_or_else(|| caps[0].to_string())
                })
                .to_string();
            if next == result {
                break;
            }
            result = next;
        }
        result
    }

    /// Resolve magic variables ($timestamp, $uuid, $date, $randomInt).
    fn resolve_magic_var(name: &str) -> Option<String> {
        use std::time::{SystemTime, UNIX_EPOCH};
        match name {
            "$timestamp" => {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let rnd: u64 = rand::random::<u64>() % 900000 + 100000;
                Some(format!("{}{}", ts, rnd))
            }
            "$uuid" => {
                let uuid = uuid::Uuid::new_v4();
                Some(uuid.to_string())
            }
            "$date" => {
                let now = chrono::Local::now();
                Some(now.format("%Y-%m-%d").to_string())
            }
            "$randomInt" => {
                let val: u64 = rand::random::<u64>() % 10000000;
                Some(val.to_string())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_var_resolver_empty() {
        let r = VarResolver::new();
        assert_eq!(r.resolve("anything"), None);
    }

    #[test]
    fn test_var_resolver_import_params_highest_priority() {
        let r = VarResolver::new()
            .with_import_params(HashMap::from([("key".into(), "import".into())]))
            .with_request_vars(HashMap::from([("key".into(), "request".into())]))
            .with_file_vars(HashMap::from([("key".into(), "file".into())]))
            .with_session_vars(HashMap::from([("key".into(), "session".into())]))
            .with_env(HashMap::from([("key".into(), "env".into())]));
        assert_eq!(r.resolve("key"), Some("import".to_string()));
    }

    #[test]
    fn test_var_resolver_request_vars_second_priority() {
        let r = VarResolver::new()
            .with_request_vars(HashMap::from([("key".into(), "request".into())]))
            .with_file_vars(HashMap::from([("key".into(), "file".into())]))
            .with_session_vars(HashMap::from([("key".into(), "session".into())]))
            .with_env(HashMap::from([("key".into(), "env".into())]));
        assert_eq!(r.resolve("key"), Some("request".to_string()));
    }

    #[test]
    fn test_var_resolver_file_vars_third_priority() {
        let r = VarResolver::new()
            .with_file_vars(HashMap::from([("key".into(), "file".into())]))
            .with_session_vars(HashMap::from([("key".into(), "session".into())]))
            .with_env(HashMap::from([("key".into(), "env".into())]));
        assert_eq!(r.resolve("key"), Some("file".to_string()));
    }

    #[test]
    fn test_var_resolver_session_vars_fourth_priority() {
        let r = VarResolver::new()
            .with_session_vars(HashMap::from([("key".into(), "session".into())]))
            .with_env(HashMap::from([("key".into(), "env".into())]));
        assert_eq!(r.resolve("key"), Some("session".to_string()));
    }

    #[test]
    fn test_var_resolver_script_vars_same_level_as_session() {
        let r = VarResolver::new()
            .with_session_vars(HashMap::from([("key".into(), "session".into())]))
            .with_script_vars(HashMap::from([("key".into(), "script".into())]))
            .with_env(HashMap::from([("key".into(), "env".into())]));
        // session_vars checked first, so it wins
        assert_eq!(r.resolve("key"), Some("session".to_string()));
    }

    #[test]
    fn test_var_resolver_env_vars_fifth_priority() {
        let r = VarResolver::new()
            .with_env(HashMap::from([("key".into(), "env".into())]));
        assert_eq!(r.resolve("key"), Some("env".to_string()));
    }

    #[test]
    fn test_var_resolver_magic_var_fallback() {
        let r = VarResolver::new();
        assert!(r.resolve("$timestamp").is_some());
        assert!(r.resolve("$uuid").is_some());
        assert!(r.resolve("$date").is_some());
        assert!(r.resolve("$randomInt").is_some());
    }

    #[test]
    fn test_var_resolver_fallback_to_next_layer_when_missing() {
        let r = VarResolver::new()
            .with_request_vars(HashMap::from([("b".into(), "2".into())]))
            .with_file_vars(HashMap::from([("c".into(), "3".into())]))
            .with_session_vars(HashMap::from([("d".into(), "4".into())]))
            .with_env(HashMap::from([("e".into(), "5".into())]));
        assert_eq!(r.resolve("a"), None);
        assert_eq!(r.resolve("b"), Some("2".to_string()));
        assert_eq!(r.resolve("c"), Some("3".to_string()));
        assert_eq!(r.resolve("d"), Some("4".to_string()));
        assert_eq!(r.resolve("e"), Some("5".to_string()));
    }

    #[test]
    fn test_var_resolver_substitute_basic() {
        let r = VarResolver::new()
            .with_env(HashMap::from([("name".into(), "World".into())]));
        assert_eq!(r.substitute("Hello, {{name}}!"), "Hello, World!");
    }

    #[test]
    fn test_var_resolver_substitute_multiple() {
        let r = VarResolver::new()
            .with_env(HashMap::from([
                ("first".into(), "Jane".into()),
                ("last".into(), "Doe".into()),
            ]));
        assert_eq!(r.substitute("{{first}} {{last}}"), "Jane Doe");
    }

    #[test]
    fn test_var_resolver_substitute_not_found_preserved() {
        let r = VarResolver::new();
        assert_eq!(r.substitute("{{missing}}"), "{{missing}}");
    }

    #[test]
    fn test_var_resolver_substitute_no_vars() {
        let r = VarResolver::new();
        assert_eq!(r.substitute("no variables"), "no variables");
    }

    #[test]
    fn test_var_resolver_substitute_magic_vars() {
        let r = VarResolver::new();
        let result = r.substitute("{{$timestamp}}");
        assert!(!result.contains("{{"), "magic var should be resolved: {}", result);
        let result = r.substitute("{{$uuid}}");
        assert!(!result.contains("{{"), "magic var should be resolved: {}", result);
    }

    #[test]
    fn test_var_resolver_priority_chain_in_substitute() {
        let r = VarResolver::new()
            .with_file_vars(HashMap::from([("host".into(), "file.com".into())]))
            .with_request_vars(HashMap::from([("host".into(), "request.com".into())]))
            .with_env(HashMap::from([("host".into(), "env.com".into())]));
        assert_eq!(r.substitute("{{host}}"), "request.com");
    }

    #[test]
    fn test_var_resolver_substitute_iterative_resolution() {
        let r = VarResolver::new()
            .with_file_vars(HashMap::from([
                ("base_url".into(), "http://{{host}}".into()),
                ("host".into(), "example.com".into()),
            ]));
        assert_eq!(r.substitute("{{base_url}}"), "http://example.com");
    }
}
