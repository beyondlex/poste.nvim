use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Check if a connection string looks like a URL (not a name).
pub fn is_connection_url(conn: &str) -> bool {
    if conn.contains("://") {
        return true;
    }
    if conn.starts_with("sqlite:") {
        return true;
    }
    if conn.starts_with('/') || conn.starts_with("./") {
        return true;
    }
    false
}

/// Find `name` in `dir` or its nearest ancestors.
pub fn find_file_upwards(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut dir = dir;
    loop {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return None,
        }
    }
}

/// Parse `.env` content: `KEY=VALUE` lines, optional `export ` prefix,
/// `#` comments, single/double-quoted values. Mirror of the Lua
/// `parse_dotenv` in poste-db's connections.lua — the two resolvers are a
/// documented mirror pair (docs/schema.md) and must not drift alone.
pub fn parse_dotenv(content: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let body = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = body.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut value = value.trim();
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = &value[1..value.len() - 1];
        }
        vars.insert(key.to_string(), value.to_string());
    }
    vars
}

/// Load the `{{var}}` substitution table: `env.json` (nearest file wins,
/// walk-up; the flat `{"KEY": "v"}` shape is tolerated like Lua's
/// `parsed.envs or parsed`) < `.env` (walk-up) < the process environment.
/// Precedence mirrors Lua `M.get_env_vars` — highest wins, applied last.
pub fn load_env_vars(search_dir: &Path, env_name: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();

    if let Some(path) = find_file_upwards(search_dir, "env.json") {
        if let Ok(content) = std::fs::read_to_string(&path) {
            match poste_core::Environment::parse(&content) {
                // nested form: only the selected environment's section
                Ok(env_file) => {
                    if let Some(section) = env_file.envs.get(env_name) {
                        vars.extend(section.clone());
                    }
                }
                // flat form: vars apply to every environment
                Err(_) => {
                    if let Ok(flat) = serde_json::from_str::<HashMap<String, String>>(&content) {
                        vars.extend(flat);
                    }
                }
            }
        }
    }

    if let Some(path) = find_file_upwards(search_dir, ".env") {
        if let Ok(content) = std::fs::read_to_string(&path) {
            vars.extend(parse_dotenv(&content));
        }
    }

    // The real OS environment wins over both files (to_string_lossy: a
    // non-UTF-8 var must not panic the CLI).
    for (k, v) in std::env::vars_os() {
        vars.insert(
            k.to_string_lossy().into_owned(),
            v.to_string_lossy().into_owned(),
        );
    }

    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dotenv_basic_and_quotes() {
        let mut vars = parse_dotenv(
            "A=1\n# comment\nexport B=two\nC=\"quoted value\"\nD='single'\n\nBAD LINE\n=",
        );
        assert_eq!(vars.remove("A").as_deref(), Some("1"));
        assert_eq!(vars.remove("B").as_deref(), Some("two"));
        assert_eq!(vars.remove("C").as_deref(), Some("quoted value"));
        assert_eq!(vars.remove("D").as_deref(), Some("single"));
        assert!(vars.is_empty(), "no phantom keys: {vars:?}");
    }

    #[test]
    fn test_parse_dotenv_value_may_contain_hash_and_equals() {
        let vars = parse_dotenv("URL=http://x/?a=1#frag\nPW=p#s=w");
        assert_eq!(
            vars.get("URL").map(String::as_str),
            Some("http://x/?a=1#frag")
        );
        assert_eq!(vars.get("PW").map(String::as_str), Some("p#s=w"));
    }

    #[test]
    fn test_find_file_upwards() {
        let base = std::env::temp_dir().join(format!("poste_util_{}", std::process::id()));
        let deep = base.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(base.join("env.json"), "{}").unwrap();
        assert_eq!(
            find_file_upwards(&deep, "env.json"),
            Some(base.join("env.json"))
        );
        assert_eq!(find_file_upwards(&deep, "missing.file"), None);
        std::fs::remove_dir_all(&base).ok();
    }
}
