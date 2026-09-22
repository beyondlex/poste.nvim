//! SQL connection configuration management.
//!
//! Connections live in `connections.toml` (the documented, sibling-shared
//! format, discovered by walking up the directory tree from the SQL file's
//! location — same discovery the Lua plugins run) with a legacy
//! `connections.json` fallback.

use anyhow::Result;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use poste_core::substitute_vars;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Percent-encode userinfo (user/password) per RFC 3986: every byte outside
/// the unreserved set (`A-Za-z0-9-._~`) is encoded, including non-ASCII UTF-8.
/// Decoded again by URL parsers, so raw config values always round-trip.
const USERINFO_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Normalize a SQLite connection string to `sqlite:<path>[?mode=rwc]` format.
/// Handles `sqlite:///`, `sqlite://`, `sqlite:`, plain paths, and `:memory:`.
/// The create-if-missing flag is appended here (not just by `to_url`) so
/// exec-file/session given a plain `sqlite:/new/file.db` actually create the
/// file instead of failing with sqlx's default mode=rw "unable to open".
pub fn normalize_sqlite_connection(conn: &str) -> anyhow::Result<String> {
    let conn = conn.trim();

    if conn.starts_with("sqlite:") && !conn.starts_with("sqlite://") {
        let path = conn.strip_prefix("sqlite:").unwrap_or(conn);
        if path == ":memory:" || path == "/:memory:" {
            return Ok("sqlite::memory:".to_string());
        }
        if path.is_empty() {
            anyhow::bail!("Invalid SQLite connection string: {}", conn)
        }
        return Ok(format!("sqlite:{}", ensure_sqlite_create_flag(path)));
    }

    if let Some(rest) = conn.strip_prefix("sqlite:///") {
        if rest == ":memory:" {
            return Ok("sqlite::memory:".to_string());
        }
        return Ok(format!(
            "sqlite:{}",
            ensure_sqlite_create_flag(&format!("/{}", rest))
        ));
    }

    if let Some(rest) = conn.strip_prefix("sqlite://") {
        return Ok(format!("sqlite:{}", ensure_sqlite_create_flag(rest)));
    }

    if conn.starts_with('/') || conn.starts_with("./") {
        return Ok(format!("sqlite:{}", ensure_sqlite_create_flag(conn)));
    }

    if conn == ":memory:" {
        return Ok("sqlite::memory:".to_string());
    }

    anyhow::bail!("Invalid SQLite connection string: {}", conn)
}

/// A single connection configuration entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Database dialect: "postgres", "mysql", or "sqlite"
    pub dialect: String,

    /// Host for network databases (postgres/mysql)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// Port for network databases (defaults based on dialect)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// `port` exactly as written, kept when it is not already a usable port
    /// number. Two reasons to defer rather than reject at parse time: a
    /// `{{var}}` reference is only resolvable once the environment is known
    /// (Lua's `apply_env` substitutes every field, `port` included, before
    /// validating it), and a genuinely broken `port` should fail the one
    /// connection that has it instead of the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_raw: Option<String>,

    /// Database name for network databases
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,

    /// Username for authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// Password for authentication (may contain {{var}} references)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// File path for SQLite databases
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// SSL mode for PostgreSQL (disable, require, prefer, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssl_mode: Option<String>,

    /// Extra connection parameters
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra_params: HashMap<String, String>,
}

/// Dialect aliases that normalize to a base dialect before any handling.
/// Mirror of Lua `poste-db/constants.lua` `DIALECT_ALIASES` — the two
/// name→URL resolvers are a documented mirror pair (docs/schema.md) and must
/// not drift alone. Unknown names pass through unchanged.
fn normalize_dialect(dialect: &str) -> &str {
    match dialect {
        "mariadb" | "tidb" | "singlestore" | "aurora-mysql" | "vitess" | "planetscale" => "mysql",
        "postgresql" | "cockroachdb" | "yugabyte" | "aurora-postgres" | "neon" | "supabase"
        | "timescaledb" => "postgres",
        other => other,
    }
}

/// sqlx 0.8 defaults sqlite URLs to mode=rw (no create), so a plain
/// `sqlite:/new/file.db` connection fails with "unable to open database
/// file". Append the create-if-missing flag without corrupting a path that
/// already carries a query string (`f.db?cache=shared` gains `&mode=rwc`; a
/// path that pins `mode=` is left untouched). Shared by `to_url` and
/// `normalize_sqlite_connection` so every sqlite entry point creates files.
fn ensure_sqlite_create_flag(path: &str) -> String {
    if path.contains('?') {
        if path.contains("mode=") {
            path.to_string()
        } else {
            format!("{}&mode=rwc", path)
        }
    } else {
        format!("{}?mode=rwc", path)
    }
}

/// IPv6 literals must be bracketed to be legal in a URL authority
/// (RFC 3986 §3.2.2), and the drivers' URL parsers enforce it:
/// `postgres://::1:5432/db` is refused as `error with configuration: empty
/// host` before a socket is opened, so a plain `host = "::1"` in
/// connections.toml was unusable. Only hex digits plus colons count as an
/// address, so the common mistake of leaving the port in the host field
/// (`localhost:5432`) is not rewritten into something else. An already
/// bracketed host passes through — that is the form that worked.
/// Mirror of Lua `poste-db/connections.lua` `url_host` (same documented pair
/// as `to_url` / `build_conn_url`).
fn url_host(host: &str) -> String {
    if host.starts_with('[') {
        return host.to_string();
    }
    let looks_like_ipv6 =
        host.contains(':') && host.chars().all(|c| c.is_ascii_hexdigit() || c == ':');
    if looks_like_ipv6 {
        return format!("[{}]", host);
    }
    host.to_string()
}

impl ConnectionConfig {
    /// Build a connection URL from the config.
    /// For SQLite, returns `sqlite:<path>[?mode=rwc]`.
    /// For Postgres/MySQL/MSSQL/ClickHouse, builds the standard URL format.
    pub fn to_url(&self) -> String {
        match normalize_dialect(&self.dialect) {
            "sqlite" => {
                let path = self.path.as_deref().unwrap_or(":memory:");
                if path == ":memory:" {
                    return "sqlite::memory:".to_string();
                }
                format!("sqlite:{}", ensure_sqlite_create_flag(path))
            }
            "postgres" | "mysql" | "mssql" | "clickhouse" => {
                let scheme = normalize_dialect(&self.dialect);
                let host = self.host.as_deref().unwrap_or("localhost");
                let default_port = match scheme {
                    "postgres" => 5432,
                    "mysql" => 3306,
                    "clickhouse" => 8123,
                    _ => 1433,
                };
                let port = self.port.unwrap_or(default_port);
                // percent-encode the db like the user/password fields — a db
                // name with `/`, spaces or unicode must not break the URL
                let db = utf8_percent_encode(
                    self.database.as_deref().unwrap_or(""),
                    USERINFO_ENCODE_SET,
                );

                let auth = match (&self.user, &self.password) {
                    (Some(u), Some(p)) => format!(
                        "{}:{}@",
                        utf8_percent_encode(u, USERINFO_ENCODE_SET),
                        utf8_percent_encode(p, USERINFO_ENCODE_SET)
                    ),
                    (Some(u), None) => format!("{}@", utf8_percent_encode(u, USERINFO_ENCODE_SET)),
                    _ => String::new(),
                };

                format!("{}://{}{}:{}/{}", scheme, auth, url_host(host), port, db)
            }
            _ => String::new(),
        }
    }

    /// Return a copy with every `{{var}}` reference resolved — `port`
    /// included. One function on purpose: `ConnectionStore::resolve` and the
    /// CLI's `connection test` used to carry their own field lists, and the
    /// two had already drifted (neither substituted `port`), so the editor
    /// and the CLI could end up on different servers from one file.
    /// Errors on a deferred `port` that never becomes a port number — either
    /// a reference the environment does not define or a value that was never
    /// one. The value is not echoed, since `port = "{{POSTE_PASS}}"` is a
    /// plausible typo and this message reaches a terminal.
    pub fn with_vars_resolved(
        &self,
        name: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<Self> {
        let sub = |s: Option<String>| s.map(|s| substitute_vars(&s, env_vars));
        let mut resolved = self.clone();
        resolved.host = sub(resolved.host);
        resolved.password = sub(resolved.password);
        resolved.user = sub(resolved.user);
        resolved.database = sub(resolved.database);
        resolved.path = sub(resolved.path);
        if let Some(raw) = resolved.port_raw.take() {
            match parse_port(&substitute_vars(&raw, env_vars)) {
                Some(n) => resolved.port = Some(n),
                None => anyhow::bail!(
                    "connection '{}' has a port that is not a number between 1 and 65535",
                    name
                ),
            }
        }
        Ok(resolved)
    }
}

/// Store for loading and resolving connection configurations.
pub struct ConnectionStore {
    connections: HashMap<String, ConnectionConfig>,
    source_path: Option<PathBuf>,
}

impl ConnectionStore {
    /// Create an empty store.
    pub fn empty() -> Self {
        Self {
            connections: HashMap::new(),
            source_path: None,
        }
    }

    /// Create a store from pre-loaded connections (no I/O, for testing).
    pub fn for_test(connections: HashMap<String, ConnectionConfig>) -> Self {
        Self {
            connections,
            source_path: None,
        }
    }

    /// Load the connection store walking up from `search_dir`. Reads the
    /// documented `connections.toml` (the same file the sibling plugins
    /// resolve — the mirror-implementation contract in docs/schema.md) and
    /// falls back to a legacy `connections.json` when no TOML exists.
    pub fn load(search_dir: &Path) -> Result<Self> {
        let config_path = find_connections_file(search_dir);

        match config_path {
            Some(path) => {
                let content = std::fs::read_to_string(&path)?;
                let is_toml = path.extension().and_then(|e| e.to_str()) == Some("toml");
                let mut connections: HashMap<String, ConnectionConfig> = if is_toml {
                    connections_from_toml(&content)?
                } else {
                    serde_json::from_str(&content)?
                };
                // Normalize dialect aliases (postgresql → postgres,
                // mariadb → mysql, …) — mirror of the Lua resolver
                for conn in connections.values_mut() {
                    conn.dialect = normalize_dialect(&conn.dialect).to_string();
                    // The legacy JSON path types `port` as a bare u16, so 0 is
                    // the one unusable value serde lets through (the TOML path
                    // goes through `parse_port`). Defer it the same way so
                    // `resolve` refuses it by name instead of building `…:0/…`.
                    if conn.port == Some(0) {
                        conn.port_raw = Some("0".to_string());
                        conn.port = None;
                    }
                }
                Ok(Self {
                    connections,
                    source_path: Some(path),
                })
            }
            None => Ok(Self::empty()),
        }
    }

    /// Get a connection config by name.
    pub fn get(&self, name: &str) -> Option<&ConnectionConfig> {
        self.connections.get(name)
    }

    /// Get all connection names.
    pub fn names(&self) -> Vec<&String> {
        self.connections.keys().collect()
    }

    /// Get all connections.
    pub fn all(&self) -> &HashMap<String, ConnectionConfig> {
        &self.connections
    }

    /// Check if a connection name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.connections.contains_key(name)
    }

    /// Resolve a connection name to a URL, substituting environment variables.
    /// Returns the connection URL string.
    pub fn resolve(&self, name: &str, env_vars: &HashMap<String, String>) -> Result<String> {
        let config = self.connections.get(name).ok_or_else(|| {
            anyhow::anyhow!("Connection '{}' not found in the connection store", name)
        })?;

        Ok(config.with_vars_resolved(name, env_vars)?.to_url())
    }

    /// Get the source file path.
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// List connections as JSON-serializable items for CLI output.
    pub fn to_json_list(&self) -> Vec<serde_json::Value> {
        let mut items: Vec<_> = self
            .connections
            .iter()
            .map(|(name, config)| {
                serde_json::json!({
                    "name": name,
                    "dialect": config.dialect,
                    "host": config.host,
                    "port": config.port,
                    "database": config.database,
                    "path": config.path,
                })
            })
            .collect();
        items.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });
        items
    }
}

/// Find the connection store file walking up from search_dir. `connections.toml`
/// is the documented, sibling-shared format and wins when a directory carries
/// both; `connections.json` remains a legacy fallback.
fn find_connections_file(search_dir: &Path) -> Option<PathBuf> {
    const NAMES: [&str; 2] = ["connections.toml", "connections.json"];
    let mut dir = search_dir.to_path_buf();
    loop {
        for name in NAMES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Parse a `port` as written in `connections.toml` — an integer or a numeric
/// string — bounded to a real socket port. It goes through `f64` because that
/// is the set Lua's `tonumber` accepts (so `"5432.0"` is a port here too, and
/// `"5432.5"` is not), and the range is checked explicitly rather than left to
/// `u16::try_from`: port 0 fits a `u16` and is not a socket port.
fn parse_port(raw: &str) -> Option<u16> {
    let value = raw.trim().parse::<f64>().ok()?;
    if value.floor() != value || !(1.0..=65535.0).contains(&value) {
        return None;
    }
    Some(value as u16)
}

/// Parse `connections.toml` into the store map. Field extraction is
/// deliberately tolerant — the same tolerance the Lua resolver applies:
/// top-level scalars (`description = "…"` above the first section) and
/// non-connection tables (satellite sections, a `tunnel` sub-table) are
/// ignored, `port` accepts both integer and string, and a missing `dialect`
/// defaults to `postgres` (Lua's `is_sql_dialect(nil) == true` rule). A
/// `port` that is not yet a usable port number is not rejected here either —
/// it is kept verbatim on the connection so `with_vars_resolved` can decide
/// once the environment is known, and report it against that one name.
fn connections_from_toml(content: &str) -> Result<HashMap<String, ConnectionConfig>> {
    let root: toml::Value = toml::from_str(content)
        .map_err(|e| anyhow::anyhow!("connections.toml parse error: {}", e))?;
    let Some(root_table) = root.as_table() else {
        return Ok(HashMap::new());
    };
    let mut connections = HashMap::new();
    for (name, entry) in root_table {
        let Some(table) = entry.as_table() else {
            continue; // top-level scalar — not a connection
        };
        let string_field = |key: &str| -> Option<String> {
            table.get(key).and_then(|v| v.as_str()).map(String::from)
        };
        let raw_port = table.get("port").map(|v| match v {
            toml::Value::Integer(n) => n.to_string(),
            toml::Value::String(s) => s.clone(),
            // `port = true`, a table, …: anything that can never parse as a
            // port, kept so `resolve` reports it against the connection name
            other => other.to_string(),
        });
        let (port, port_raw) = match &raw_port {
            Some(raw) => match parse_port(raw) {
                Some(n) => (Some(n), None),
                None => (None, Some(raw.clone())),
            },
            None => (None, None),
        };
        let config = ConnectionConfig {
            // Missing dialect behaves like postgres, mirroring the Lua
            // resolver's nil-dialect default.
            dialect: string_field("dialect").unwrap_or_else(|| "postgres".to_string()),
            host: string_field("host"),
            port,
            port_raw,
            database: string_field("database"),
            user: string_field("user"),
            password: string_field("password"),
            path: string_field("path"),
            ssl_mode: string_field("ssl_mode"),
            extra_params: HashMap::new(),
        };
        connections.insert(name.clone(), config);
    }
    Ok(connections)
}

/// Test a connection by attempting to connect.
pub async fn test_connection(config: &ConnectionConfig) -> Result<String> {
    let url = config.to_url();

    match config.dialect.as_str() {
        "postgres" => {
            let pool: sqlx::Pool<sqlx::Postgres> = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(&url)
                .await?;
            pool.close().await;
            Ok("OK".to_string())
        }
        "mysql" => {
            let pool: sqlx::Pool<sqlx::MySql> = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(&url)
                .await?;
            pool.close().await;
            Ok("OK".to_string())
        }
        "sqlite" => {
            let pool: sqlx::Pool<sqlx::Sqlite> = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await?;
            pool.close().await;
            Ok("OK".to_string())
        }
        "mssql" => {
            let mut client = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                crate::sql_executor::mssql::connect_mssql(&url),
            )
            .await
            .map_err(|_| anyhow::anyhow!("MSSQL connection timed out"))??;
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client.simple_query("SELECT 1"),
            )
            .await
            .map_err(|_| anyhow::anyhow!("MSSQL probe timed out"))??;
            Ok("OK".to_string())
        }
        "clickhouse" => {
            let client = crate::sql_executor::clickhouse::connect_clickhouse(&url).await?;
            let _ =
                crate::sql_executor::clickhouse::clickhouse_post(&client, "SELECT 1", 5).await?;
            Ok("OK".to_string())
        }
        other => anyhow::bail!("Unknown dialect: {}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_config_postgres_url() {
        let config = ConnectionConfig {
            dialect: "postgres".to_string(),
            host: Some("localhost".to_string()),
            port: Some(5432),
            database: Some("myapp".to_string()),
            user: Some("admin".to_string()),
            password: Some("secret".to_string()),
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(
            config.to_url(),
            "postgres://admin:secret@localhost:5432/myapp"
        );
    }

    #[test]
    fn test_connection_config_url_encodes_special_chars() {
        let config = ConnectionConfig {
            dialect: "postgres".to_string(),
            host: Some("localhost".to_string()),
            port: Some(5432),
            database: Some("blog".to_string()),
            user: Some("alice".to_string()),
            password: Some("p@ss:w/rd%".to_string()),
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(
            config.to_url(),
            "postgres://alice:p%40ss%3Aw%2Frd%25@localhost:5432/blog"
        );
    }

    #[test]
    fn test_connection_config_url_encodes_user() {
        let config = ConnectionConfig {
            dialect: "postgres".to_string(),
            host: Some("localhost".to_string()),
            port: Some(5432),
            database: Some("blog".to_string()),
            user: Some("user@example.com".to_string()),
            password: Some("pw".to_string()),
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(
            config.to_url(),
            "postgres://user%40example.com:pw@localhost:5432/blog"
        );
    }

    #[test]
    fn test_connection_config_postgres_default_port() {
        let config = ConnectionConfig {
            dialect: "postgres".to_string(),
            host: Some("db.example.com".to_string()),
            port: None,
            database: Some("prod".to_string()),
            user: Some("user".to_string()),
            password: None,
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(config.to_url(), "postgres://user@db.example.com:5432/prod");
    }

    #[test]
    fn test_connection_config_mysql_url() {
        let config = ConnectionConfig {
            dialect: "mysql".to_string(),
            host: Some("127.0.0.1".to_string()),
            port: Some(3306),
            database: Some("staging".to_string()),
            user: Some("root".to_string()),
            password: Some("pass123".to_string()),
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(
            config.to_url(),
            "mysql://root:pass123@127.0.0.1:3306/staging"
        );
    }

    #[test]
    fn test_connection_config_sqlite_url() {
        let config = ConnectionConfig {
            dialect: "sqlite".to_string(),
            host: None,
            port: None,
            database: None,
            user: None,
            password: None,
            path: Some("./data/app.db".to_string()),
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(config.to_url(), "sqlite:./data/app.db?mode=rwc");
    }

    #[test]
    fn test_connection_config_sqlite_memory() {
        let config = ConnectionConfig {
            dialect: "sqlite".to_string(),
            host: None,
            port: None,
            database: None,
            user: None,
            password: None,
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(config.to_url(), "sqlite::memory:");
    }

    #[test]
    fn test_substitute_vars() {
        let mut vars = HashMap::new();
        vars.insert("db_host".to_string(), "localhost".to_string());
        vars.insert("db_pass".to_string(), "secret".to_string());

        assert_eq!(substitute_vars("{{db_host}}", &vars), "localhost");
        assert_eq!(
            substitute_vars("host={{db_host}} pass={{db_pass}}", &vars),
            "host=localhost pass=secret"
        );
        assert_eq!(substitute_vars("{{missing}}", &vars), "{{missing}}");
    }

    #[test]
    fn test_connection_store_empty() {
        let store = ConnectionStore::empty();
        assert!(!store.contains("anything"));
        assert!(store.get("anything").is_none());
        assert!(store.names().is_empty());
    }

    #[test]
    fn test_connection_store_resolve() {
        let mut connections = HashMap::new();
        connections.insert(
            "dev-pg".to_string(),
            ConnectionConfig {
                dialect: "postgres".to_string(),
                host: Some("{{db_host}}".to_string()),
                port: Some(5432),
                database: Some("myapp".to_string()),
                user: Some("admin".to_string()),
                password: Some("{{db_pass}}".to_string()),
                path: None,
                ssl_mode: None,
                port_raw: None,
                extra_params: HashMap::new(),
            },
        );

        let store = ConnectionStore {
            connections,
            source_path: None,
        };

        let mut env_vars = HashMap::new();
        env_vars.insert("db_host".to_string(), "localhost".to_string());
        env_vars.insert("db_pass".to_string(), "secret".to_string());

        let url = store.resolve("dev-pg", &env_vars).unwrap();
        assert_eq!(url, "postgres://admin:secret@localhost:5432/myapp");
    }

    #[test]
    fn test_connection_store_resolve_missing() {
        let store = ConnectionStore::empty();
        let result = store.resolve("nonexistent", &HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn test_find_connections_file_walks_up() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sub_dir = temp_dir.path().join("sub").join("deep");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let config_path = temp_dir.path().join("connections.json");
        std::fs::write(
            &config_path,
            r#"{"test": {"dialect": "sqlite", "path": "test.db"}}"#,
        )
        .unwrap();

        // Search from deep subdirectory should find it
        let found = find_connections_file(&sub_dir);
        assert_eq!(found.unwrap(), config_path);
    }

    #[test]
    fn test_to_json_list() {
        let mut connections = HashMap::new();
        connections.insert(
            "dev-pg".to_string(),
            ConnectionConfig {
                dialect: "postgres".to_string(),
                host: Some("localhost".to_string()),
                port: Some(5432),
                database: Some("myapp".to_string()),
                user: None,
                password: None,
                path: None,
                ssl_mode: None,
                port_raw: None,
                extra_params: HashMap::new(),
            },
        );
        connections.insert(
            "local-sqlite".to_string(),
            ConnectionConfig {
                dialect: "sqlite".to_string(),
                host: None,
                port: None,
                database: None,
                user: None,
                password: None,
                path: Some("./data.db".to_string()),
                ssl_mode: None,
                port_raw: None,
                extra_params: HashMap::new(),
            },
        );

        let store = ConnectionStore {
            connections,
            source_path: None,
        };

        let list = store.to_json_list();
        assert_eq!(list.len(), 2);
        // Should be sorted by name
        assert_eq!(list[0]["name"], "dev-pg");
        assert_eq!(list[1]["name"], "local-sqlite");
    }

    #[test]
    fn test_normalize_sqlite_absolute_path() {
        assert_eq!(
            super::normalize_sqlite_connection("sqlite:///home/user/db.sqlite").unwrap(),
            "sqlite:/home/user/db.sqlite?mode=rwc"
        );
    }

    #[test]
    fn test_normalize_sqlite_relative_path() {
        assert_eq!(
            super::normalize_sqlite_connection("sqlite://./data.db").unwrap(),
            "sqlite:./data.db?mode=rwc"
        );
        assert_eq!(
            super::normalize_sqlite_connection("sqlite://data.db").unwrap(),
            "sqlite:data.db?mode=rwc"
        );
    }

    #[test]
    fn test_normalize_sqlite_memory() {
        assert_eq!(
            super::normalize_sqlite_connection("sqlite::memory:").unwrap(),
            "sqlite::memory:"
        );
        assert_eq!(
            super::normalize_sqlite_connection(":memory:").unwrap(),
            "sqlite::memory:"
        );
    }

    #[test]
    fn test_normalize_sqlite_plain_path() {
        assert_eq!(
            super::normalize_sqlite_connection("/absolute/path.db").unwrap(),
            "sqlite:/absolute/path.db?mode=rwc"
        );
        assert_eq!(
            super::normalize_sqlite_connection("./relative.db").unwrap(),
            "sqlite:./relative.db?mode=rwc"
        );
    }

    #[test]
    fn test_normalize_sqlite_already_correct() {
        assert_eq!(
            super::normalize_sqlite_connection("sqlite:/path.db").unwrap(),
            "sqlite:/path.db?mode=rwc"
        );
    }

    #[test]
    fn test_normalize_sqlite_keeps_existing_query_and_pinned_mode() {
        // an existing query string gains &mode=rwc…
        assert_eq!(
            super::normalize_sqlite_connection("sqlite:f.db?cache=shared").unwrap(),
            "sqlite:f.db?cache=shared&mode=rwc"
        );
        // …and a pinned mode= is never duplicated
        assert_eq!(
            super::normalize_sqlite_connection("sqlite:f.db?mode=ro").unwrap(),
            "sqlite:f.db?mode=ro"
        );
    }

    #[test]
    fn test_to_url_normalizes_dialect_aliases() {
        // mirror parity with Lua DIALECT_ALIASES: an alias must build the
        // base-dialect URL, never the silent empty-URL fallback
        let mut config = ConnectionConfig {
            dialect: "postgresql".to_string(),
            host: Some("localhost".to_string()),
            port: None,
            database: Some("db".to_string()),
            user: None,
            password: None,
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(config.to_url(), "postgres://localhost:5432/db");
        config.dialect = "mariadb".to_string();
        assert_eq!(config.to_url(), "mysql://localhost:3306/db");
        config.dialect = "cockroachdb".to_string();
        assert_eq!(config.to_url(), "postgres://localhost:5432/db");
        // unknown dialects still pass through (empty URL, as before)
        config.dialect = "redis".to_string();
        assert_eq!(config.to_url(), "");
    }

    #[test]
    fn test_to_url_sqlite_keeps_existing_query_string() {
        let mut config = ConnectionConfig {
            dialect: "sqlite".to_string(),
            host: None,
            port: None,
            database: None,
            user: None,
            password: None,
            path: Some("./data/app.db?cache=shared".to_string()),
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(
            config.to_url(),
            "sqlite:./data/app.db?cache=shared&mode=rwc"
        );
        // a pinned mode= is not duplicated
        config.path = Some("./data/app.db?mode=rw".to_string());
        assert_eq!(config.to_url(), "sqlite:./data/app.db?mode=rw");
    }

    #[test]
    fn test_to_url_percent_encodes_database() {
        let config = ConnectionConfig {
            dialect: "postgres".to_string(),
            host: Some("localhost".to_string()),
            port: None,
            database: Some("my db/prod".to_string()),
            user: None,
            password: None,
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        assert_eq!(config.to_url(), "postgres://localhost:5432/my%20db%2Fprod");
    }

    #[test]
    fn test_to_url_brackets_ipv6_hosts() {
        let with_host = |host: &str| ConnectionConfig {
            dialect: "postgres".to_string(),
            host: Some(host.to_string()),
            port: Some(5432),
            database: Some("db".to_string()),
            user: None,
            password: None,
            path: None,
            ssl_mode: None,
            port_raw: None,
            extra_params: HashMap::new(),
        };
        // Brackets are what the driver's URL parser needs: without them
        // `postgres://::1:5432/db` is refused as `empty host` (measured against
        // `poste introspect`) before any socket is opened.
        assert_eq!(with_host("::1").to_url(), "postgres://[::1]:5432/db");
        assert_eq!(
            with_host("fe80::1").to_url(),
            "postgres://[fe80::1]:5432/db"
        );
        // Already bracketed passes through; a port left in the host field is a
        // config mistake this must not rewrite into a different string.
        for host in [
            "[::1]",
            "localhost:5432",
            "db.internal",
            "10.0.0.1",
            "abcdef",
        ] {
            assert_eq!(
                with_host(host).to_url(),
                format!("postgres://{}:5432/db", host)
            );
        }
    }

    // ---- connections.toml store (the documented sibling-shared format) ----

    fn write_toml(dir: &Path, content: &str) {
        std::fs::write(dir.join("connections.toml"), content).unwrap();
    }

    #[test]
    fn store_loads_toml_and_resolves_the_same_urls_as_lua() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            r#"
description = "shared connections file"

[primary]
dialect = "postgres"
host = "db.internal"
port = 5432
database = "prod"
user = "alice"
password = "p@ss"

[audit]
dialect = "mariadb"
host = "db1"
database = "web"

[files]
dialect = "sqlite"
path = "./data/app.db"

[tunnelled]
dialect = "postgres"
host = "10.0.0.5"
port = "5433"
database = "t"
tunnel = { jump = "bastion", remote_port = 5432 }
"#,
        );
        let store = ConnectionStore::load(dir.path()).unwrap();

        // mariadb normalizes to mysql, tunnel sub-table ignored, string port parsed
        assert_eq!(store.names().len(), 4);
        let primary = store.resolve("primary", &Default::default()).unwrap();
        assert_eq!(primary, "postgres://alice:p%40ss@db.internal:5432/prod");
        let audit = store.resolve("audit", &Default::default()).unwrap();
        assert_eq!(audit, "mysql://db1:3306/web");
        let files = store.resolve("files", &Default::default()).unwrap();
        assert_eq!(files, "sqlite:./data/app.db?mode=rwc");
        let tunnelled = store.resolve("tunnelled", &Default::default()).unwrap();
        assert_eq!(tunnelled, "postgres://10.0.0.5:5433/t");
        assert_eq!(
            store.source_path().unwrap().file_name().unwrap(),
            "connections.toml"
        );
    }

    #[test]
    fn store_toml_missing_dialect_defaults_to_postgres() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            r#"
[legacy]
host = "old-db"
database = "main"
"#,
        );
        let store = ConnectionStore::load(dir.path()).unwrap();
        assert_eq!(
            store.resolve("legacy", &Default::default()).unwrap(),
            "postgres://old-db:5432/main"
        );
    }

    #[test]
    fn store_prefers_toml_over_legacy_json_in_the_same_directory() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            r#"
[a]
dialect = "postgres"
host = "toml-host"
"#,
        );
        std::fs::write(
            dir.path().join("connections.json"),
            r#"{"a": {"dialect": "postgres", "host": "json-host"}}"#,
        )
        .unwrap();
        let store = ConnectionStore::load(dir.path()).unwrap();
        assert_eq!(
            store.resolve("a", &Default::default()).unwrap(),
            "postgres://toml-host:5432/"
        );
    }

    #[test]
    fn store_json_still_loads_when_no_toml_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("connections.json"),
            r#"{"b": {"dialect": "mysql", "host": "h", "port": 3307}}"#,
        )
        .unwrap();
        let store = ConnectionStore::load(dir.path()).unwrap();
        assert_eq!(
            store.resolve("b", &Default::default()).unwrap(),
            "mysql://h:3307/"
        );
    }

    #[test]
    fn store_json_port_zero_is_refused_like_tomls_is() {
        // `port` in the legacy JSON file is a bare u16, so 0 — not a socket
        // port — was the one unusable value that survived the load
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("connections.json"),
            r#"{"b": {"dialect": "mysql", "host": "h", "port": 0}}"#,
        )
        .unwrap();
        let store = ConnectionStore::load(dir.path()).unwrap();
        let err = format!("{}", store.resolve("b", &Default::default()).unwrap_err());
        assert!(err.contains("port"), "says what is wrong: {err}");
        assert!(err.contains('b'), "names the connection: {err}");
    }

    #[test]
    fn store_broken_toml_is_an_error_not_silence() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            "[a
host = ",
        );
        let err = ConnectionStore::load(dir.path())
            .err()
            .expect("broken toml must fail");
        assert!(format!("{}", err).contains("connections.toml parse error"));
    }

    #[test]
    fn store_unusable_port_fails_only_that_connection_and_names_it() {
        // A `port` that is not a port number used to fall through to the
        // dialect default, so `poste connection test` checked 5432 while the
        // Lua resolver refused the same entry. Refusing it must stay scoped to
        // that connection: the file is shared, and one typo'd section taking
        // down every other connection would be the worse bug.
        for value in [
            "\"{{MISSING_PORT}}\"",
            "70000",
            "\"70000\"",
            "0",
            "\"abc\"",
            "5432.5",
        ] {
            let dir = tempfile::tempdir().unwrap();
            write_toml(
                dir.path(),
                &format!(
                    "[dev]\ndialect = \"postgres\"\nhost = \"h\"\nport = {value}\n\
                     [ok]\ndialect = \"postgres\"\nhost = \"h\"\n"
                ),
            );
            let store = ConnectionStore::load(dir.path())
                .unwrap_or_else(|e| panic!("port = {value} must not fail the load: {e}"));
            assert_eq!(
                store.resolve("ok", &Default::default()).unwrap(),
                "postgres://h:5432/",
                "a sibling connection still resolves"
            );
            let err = store
                .resolve("dev", &Default::default())
                .err()
                .unwrap_or_else(|| panic!("port = {value} must fail the resolve"));
            let msg = format!("{}", err);
            assert!(msg.contains("dev"), "message names the connection: {msg}");
            assert!(msg.contains("port"), "message says what is wrong: {msg}");
            assert!(
                !msg.contains("MISSING_PORT") && !msg.contains("70000"),
                "the offending value stays out of the message: {msg}"
            );
        }
    }

    #[test]
    fn store_port_comes_from_the_environment_like_the_editor_does() {
        // Lua's `apply_env` substitutes `port` before validating it, so
        // `port = "{{DB_PORT}}"` is a supported config. Dropping it here sent
        // the CLI to 5432 while the editor connected to the real port.
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            "[dev]\ndialect = \"postgres\"\nhost = \"h\"\nport = \"{{DB_PORT}}\"\n",
        );
        let store = ConnectionStore::load(dir.path()).unwrap();
        let mut vars = HashMap::new();
        vars.insert("DB_PORT".to_string(), "6000".to_string());
        assert_eq!(
            store.resolve("dev", &vars).unwrap(),
            "postgres://h:6000/",
            "an env-sourced port must reach the URL, not the dialect default"
        );
    }

    #[test]
    fn store_accepts_a_quoted_port_and_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            "[a]\ndialect = \"mysql\"\nhost = \"h\"\nport = \"3307\"\n[b]\ndialect = \"mysql\"\nhost = \"h\"\n",
        );
        let store = ConnectionStore::load(dir.path()).unwrap();
        assert_eq!(
            store.resolve("a", &Default::default()).unwrap(),
            "mysql://h:3307/"
        );
        assert_eq!(
            store.resolve("b", &Default::default()).unwrap(),
            "mysql://h:3306/"
        );
    }
}
