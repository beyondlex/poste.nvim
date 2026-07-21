use anyhow::{Context, Result};
use clap::Parser;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Execute a request at a specific line
#[derive(Parser)]
pub struct RunArgs {
    /// File path (used for env.json discovery and extension detection;
    /// with --stdin the file does not need to exist on disk)
    pub file: String,
    /// Line number (required for execution; ignored with --describe unless filtering)
    #[arg(short, long, default_value = "1")]
    pub line: usize,
    /// Environment name
    #[arg(short, long, default_value = "dev")]
    pub env: String,
    /// Output as JSON (for Neovim plugin consumption)
    #[arg(long)]
    pub json: bool,
    /// Describe all request blocks as JSON metadata (no execution).
    /// Single source of truth for block name/line/method/path/headers.
    #[arg(long)]
    pub describe: bool,
    /// Read request content from stdin instead of from the file
    #[arg(long)]
    pub stdin: bool,
    /// Override database name (for USE statement context from the editor)
    #[arg(long)]
    pub database: Option<String>,
}

pub async fn execute(args: RunArgs) -> Result<()> {
    let file_path = std::path::PathBuf::from(&args.file);

    // Determine the directory to search for env.json, and the file extension.
    // With --stdin the file may not exist on disk, so use its path as-is.
    let (search_dir, file_ext) = if args.stdin {
        let abs = if file_path.is_absolute() {
            file_path.clone()
        } else {
            std::env::current_dir()?.join(&file_path)
        };
        let ext = abs
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("http")
            .to_string();
        let dir = abs
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        (dir, ext)
    } else {
        // Resolve and canonicalize the file path
        let abs = if file_path.is_absolute() {
            file_path.clone()
        } else {
            std::env::current_dir()?.join(&file_path)
        };
        let canonical = std::fs::canonicalize(&abs)
            .map_err(|e| anyhow::anyhow!("Request file not found: {} ({})", abs.display(), e))?;
        let ext = canonical
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("http")
            .to_string();
        let dir = canonical
            .parent()
            .context("File path resolves to root")?
            .to_path_buf();
        (dir, ext)
    };

    // Find env.json: optional (SQL files with direct connection names don't need it)
    let mut dir = search_dir.as_path();
    let env_vars = loop {
        let candidate = dir.join("env.json");
        if candidate.exists() {
            let env_file = poste_core::Environment::load(
                candidate
                    .to_str()
                    .context("env.json path is not valid UTF-8")?,
            )?;
            let vars = env_file.envs.get(&args.env).cloned().unwrap_or_default();
            break vars;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break std::collections::HashMap::new(),
        }
    };

    // Read request content
    let content = if args.stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        let canonical = std::fs::canonicalize(&file_path).unwrap_or(file_path.clone());
        std::fs::read_to_string(&canonical)?
    };

    // --describe: emit block metadata JSON and exit (no network I/O).
    // Always returns a JSON array of BlockMeta — single parse authority for Lua.
    if args.describe {
        let parser = poste_core::Parser::new(env_vars);
        let blocks = parser.describe_blocks(&content, &file_ext)?;
        println!("{}", serde_json::to_string(&blocks)?);
        return Ok(());
    }

    // Parse the request
    let parser = poste_core::Parser::new(env_vars.clone());
    let mut request = parser.parse_at_line(&content, args.line, &file_ext)?;

    // Save raw body AFTER variable substitution but BEFORE file include
    // resolution — so Verbose/Rqst tabs show `{{host}}` expanded but
    // `< ./photo.png` kept as-is (not dumping binary content).
    request.raw_body = request.body_str().to_string();

    // Expand < file directives in the body (must happen after parsing so that
    // ### in file content doesn't corrupt block boundary detection).
    request.body = resolve_file_includes(request.body_str(), &search_dir)?;

    // Resolve connection name for SQL protocols
    if crate::util::is_sql_protocol(&request.protocol)
        && !crate::util::is_connection_url(&request.connection)
        && !request.connection.is_empty()
    {
        let conn_name = request.connection.clone();
        let conn_store = poste_exec::sql_connection::ConnectionStore::load(&search_dir)?;
        request.connection = conn_store
            .resolve(&conn_name, &env_vars)
            .map_err(|e| anyhow::anyhow!("Failed to resolve connection '{}': {}", conn_name, e))?;
    }

    // Override database from --database flag
    if let Some(ref db) = args.database {
        if crate::util::is_sql_protocol(&request.protocol) && !request.connection.is_empty() {
            request.connection = poste_core::replace_database_in_url(&request.connection, db);
        }
    }

    // Auto-detect protocol from connection URL for .sql files
    if request.protocol == poste_core::Protocol::Postgres
        && request.connection.starts_with("sqlite:")
    {
        request.protocol = poste_core::Protocol::Sqlite;
    }
    if request.protocol == poste_core::Protocol::Postgres
        && request.connection.starts_with("mysql://")
    {
        request.protocol = poste_core::Protocol::Mysql;
    }

    // Load cookie jar
    let cookie_jar = poste_exec::CookieJar::load(&args.env);

    // Execute
    let response = poste_exec::Executor::execute(&request, Some(&cookie_jar)).await?;

    // Save cookies (best effort)
    if let Err(e) = cookie_jar.save() {
        eprintln!("[poste] warning: failed to save cookies: {}", e);
    }

    if args.json {
        println!("{}", serde_json::to_string(&response)?);
    } else {
        println!("Executing: {:?}", request.name);
        println!("Protocol: {:?}", request.protocol);
        println!("Connection: {}", request.connection);
        println!();

        println!("Status: {}", response.status_text);
        println!("Latency: {}ms", response.latency_ms);
        println!("URL: {}", response.url);
        if !response.cookies.is_empty() {
            println!("Cookies:");
            for c in &response.cookies {
                println!(
                    "  {}={} (domain={}, path={})",
                    c.name, c.value, c.domain, c.path
                );
            }
        }
        println!("Headers:");
        for (key, value) in &response.headers {
            println!("  {}: {}", key, value);
        }
        println!();
        println!("Body:");

        if response.content_type.contains("json") {
            match serde_json::from_str::<serde_json::Value>(&response.body) {
                Ok(json) => println!("{}", serde_json::to_string_pretty(&json).unwrap()),
                Err(_) => println!("{}", response.body),
            }
        } else {
            println!("{}", response.body);
        }
    }

    Ok(())
}

/// Expand `< file` directives in the request body.
///
/// Lines matching `< path` are replaced with the file contents. Supports:
/// - relative paths (resolved against `base_dir`)
/// - `~` for home directory
/// - absolute paths
///
/// Returns an error if the included file cannot be read.
fn resolve_file_includes(body: &str, base_dir: &Path) -> Result<Vec<u8>> {
    let home = std::env::var("HOME").ok();
    let mut result = Vec::with_capacity(body.len());
    for (line_idx, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(path_str) = trimmed.strip_prefix("< ") {
            let resolved = resolve_include_path(path_str.trim(), base_dir, &home);
            match std::fs::read(&resolved) {
                Ok(bytes) => {
                    result.extend_from_slice(&bytes);
                    // file content includes its own newlines; don't add another
                }
                Err(e) => {
                    anyhow::bail!(
                        "Cannot read file '{}' included via `< {}` at line {}: {}",
                        resolved.display(),
                        path_str.trim(),
                        line_idx + 1,
                        e,
                    );
                }
            }
        } else {
            result.extend_from_slice(line.as_bytes());
            result.push(b'\n');
        }
    }
    // Preserve trailing-newline semantics of the original body
    if !body.is_empty() && !body.ends_with('\n') && result.last() == Some(&b'\n') {
        result.pop();
    }
    Ok(result)
}

/// Resolve a `< file` include path relative to `base_dir`.
fn resolve_include_path(path_str: &str, base_dir: &Path, home: &Option<String>) -> PathBuf {
    if path_str.starts_with('~') {
        if let Some(h) = home {
            PathBuf::from(h).join(path_str.strip_prefix("~/").unwrap_or(""))
        } else {
            base_dir.join(path_str)
        }
    } else if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        base_dir.join(path_str)
    }
}

/// Load env vars for variable substitution.
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
