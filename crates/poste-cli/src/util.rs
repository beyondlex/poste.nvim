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

/// Load environment variables for substitution by walking up from `search_dir`.
pub fn load_env_vars(
    search_dir: &std::path::Path,
    env_name: &str,
) -> std::collections::HashMap<String, String> {
    let mut dir = search_dir;
    loop {
        let candidate = dir.join("env.json");
        if candidate.exists() {
            if let Ok(env_file) = poste_core::Environment::load(
                candidate
                    .to_str()
                    .expect("env.json path must be valid UTF-8"),
            ) {
                if let Some(vars) = env_file.envs.get(env_name) {
                    return vars.clone();
                }
            }
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    std::collections::HashMap::new()
}
