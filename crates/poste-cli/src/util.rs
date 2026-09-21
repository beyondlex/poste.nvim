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

/// Turn a JSON object into variables, keeping every scalar.
///
/// A port written as a JSON number (`{"PORT": 5432}`, the natural thing to type
/// in a JSON file) is a variable with the value `"5432"`.  Reading the file into
/// `HashMap<String, String>` instead rejected the *whole* document for one such
/// value, so every `{{VAR}}` in the connection stayed literal.
fn scalar_vars(map: &serde_json::Map<String, serde_json::Value>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (k, v) in map {
        let value = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            // Nested objects and null are not variables; skip them so one
            // structural entry cannot hide the names beside it.
            _ => continue,
        };
        out.insert(k.clone(), value);
    }
    out
}

/// Load the `{{var}}` substitution table: `env.json` (nearest file wins,
/// walk-up; the flat `{"KEY": "v"}` shape is tolerated) < `.env` (walk-up) <
/// the process environment.  Precedence mirrors Lua `M.get_env_vars` — highest
/// wins, applied last.
pub fn load_env_vars(search_dir: &Path, env_name: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();

    if let Some(path) = find_file_upwards(search_dir, "env.json") {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                // Lua reads `parsed.envs or parsed`, then indexes the environment.
                let container = parsed.get("envs").unwrap_or(&parsed);
                let section = container.get(env_name).and_then(|s| s.as_object());
                let map = match section {
                    Some(section) => scalar_vars(section),
                    // Flat form: the names are variables for every environment.
                    None => match container.as_object() {
                        Some(container) => scalar_vars(container),
                        None => HashMap::new(),
                    },
                };
                vars.extend(map);
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("poste_env_{}_{}", std::process::id(), tag));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
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

    #[test]
    fn test_load_env_vars_selects_environment_section() {
        let dir = temp_dir("select");
        std::fs::write(
            dir.join("env.json"),
            r#"{"dev":{"POSTE_TEST_HOST":"h-dev"},"prod":{"POSTE_TEST_HOST":"h-prod"}}"#,
        )
        .unwrap();

        let dev = load_env_vars(&dir, "dev");
        let prod = load_env_vars(&dir, "prod");
        assert_eq!(
            dev.get("POSTE_TEST_HOST").map(String::as_str),
            Some("h-dev")
        );
        assert_eq!(
            prod.get("POSTE_TEST_HOST").map(String::as_str),
            Some("h-prod"),
            "the other environment's section must not leak in"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_load_env_vars_keeps_scalar_numbers_and_skips_objects() {
        // One port as a JSON number used to fail the whole document into
        // `HashMap<String, String>`, discarding even its string siblings.
        let dir = temp_dir("numbers");
        std::fs::write(
            dir.join("env.json"),
            r#"{"dev":{"POSTE_TEST_HOST":"h1","POSTE_TEST_PORT":5432,"POSTE_TEST_META":{"a":1},"POSTE_TEST_NULL":null}}"#,
        )
        .unwrap();

        let vars = load_env_vars(&dir, "dev");
        assert_eq!(vars.get("POSTE_TEST_HOST").map(String::as_str), Some("h1"));
        assert_eq!(
            vars.get("POSTE_TEST_PORT").map(String::as_str),
            Some("5432")
        );
        assert_eq!(vars.get("POSTE_TEST_META"), None, "an object is not a var");
        assert_eq!(vars.get("POSTE_TEST_NULL"), None, "null is not a var");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_load_env_vars_reads_envs_wrapper_and_flat_form() {
        let dir = temp_dir("shapes");
        std::fs::write(
            dir.join("env.json"),
            r#"{"envs":{"dev":{"POSTE_TEST_WRAPPED":"w"}}}"#,
        )
        .unwrap();
        let vars = load_env_vars(&dir, "dev");
        assert_eq!(
            vars.get("POSTE_TEST_WRAPPED").map(String::as_str),
            Some("w"),
            "`{{\"envs\": ...}}` is the shape poste-db's Lua reads"
        );

        std::fs::write(dir.join("env.json"), r#"{"POSTE_TEST_FLAT":"f"}"#).unwrap();
        let vars = load_env_vars(&dir, "anything");
        assert_eq!(vars.get("POSTE_TEST_FLAT").map(String::as_str), Some("f"));

        // Unknown environment in a nested file yields nothing rather than the
        // section names themselves.
        std::fs::write(
            dir.join("env.json"),
            r#"{"dev":{"POSTE_TEST_FLAT":"f"},"prod":{"POSTE_TEST_FLAT":"g"}}"#,
        )
        .unwrap();
        let vars = load_env_vars(&dir, "staging");
        assert_eq!(vars.get("dev"), None);
        assert_eq!(vars.get("POSTE_TEST_FLAT"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_load_env_vars_precedence_dotenv_beats_json_beats_os_env() {
        let dir = temp_dir("precedence");
        std::fs::write(
            dir.join("env.json"),
            r#"{"dev":{"POSTE_TEST_FROM_JSON":"json","POSTE_TEST_FROM_DOTENV":"json","POSTE_TEST_FROM_OS":"json"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".env"),
            "POSTE_TEST_FROM_DOTENV=dotenv\nPOSTE_TEST_FROM_OS=dotenv\n",
        )
        .unwrap();
        std::env::set_var("POSTE_TEST_FROM_OS", "os");

        let vars = load_env_vars(&dir, "dev");
        assert_eq!(
            vars.get("POSTE_TEST_FROM_JSON").map(String::as_str),
            Some("json"),
            "a name only env.json defines survives"
        );
        assert_eq!(
            vars.get("POSTE_TEST_FROM_DOTENV").map(String::as_str),
            Some("dotenv"),
            ".env wins over env.json"
        );
        assert_eq!(
            vars.get("POSTE_TEST_FROM_OS").map(String::as_str),
            Some("os"),
            "the process environment wins over both files"
        );
        std::env::remove_var("POSTE_TEST_FROM_OS");
        std::fs::remove_dir_all(&dir).ok();
    }
}
