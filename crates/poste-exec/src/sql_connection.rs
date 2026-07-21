//! SQL connection configuration management.
//!
//! Connections are stored in `connections.json` files, discovered by walking
//! up the directory tree from the SQL file's location (same as env.json).

use anyhow::Result;
use poste_core::substitute_vars;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Normalize a SQLite connection string to `sqlite:<path>` format.
/// Handles `sqlite:///`, `sqlite://`, `sqlite:`, plain paths, and `:memory:`.
pub fn normalize_sqlite_connection(conn: &str) -> anyhow::Result<String> {
    let conn = conn.trim();

    if conn.starts_with("sqlite:") && !conn.starts_with("sqlite://") {
        return Ok(conn.to_string());
    }

    if let Some(rest) = conn.strip_prefix("sqlite:///") {
        return Ok(format!("sqlite:/{}", rest));
    }

    if let Some(rest) = conn.strip_prefix("sqlite://") {
        return Ok(format!("sqlite:{}", rest));
    }

    if conn.starts_with('/') || conn.starts_with("./") || conn.starts_with(":memory:") {
        return Ok(format!("sqlite:{}", conn));
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

impl ConnectionConfig {
    /// Build a connection URL from the config.
    /// For SQLite, returns `sqlite://path`.
    /// For Postgres/MySQL, builds the standard URL format.
    pub fn to_url(&self) -> String {
        match self.dialect.as_str() {
            "sqlite" => {
                let path = self.path.as_deref().unwrap_or(":memory:");
                // sqlx expects: sqlite::memory: or sqlite:/absolute/path or sqlite:relative/path
                if path == ":memory:" {
                    "sqlite::memory:".to_string()
                } else {
                    format!("sqlite:{}", path)
                }
            }
            "postgres" | "mysql" => {
                let scheme = self.dialect.as_str();
                let host = self.host.as_deref().unwrap_or("localhost");
                let default_port = match scheme {
                    "postgres" => 5432,
                    "mysql" => 3306,
                    _ => 0,
                };
                let port = self.port.unwrap_or(default_port);
                let db = self.database.as_deref().unwrap_or("");

                let auth = match (&self.user, &self.password) {
                    (Some(u), Some(p)) => format!("{}:{}@", u, p),
                    (Some(u), None) => format!("{}@", u),
                    _ => String::new(),
                };

                format!("{}://{}{}:{}/{}", scheme, auth, host, port, db)
            }
            _ => String::new(),
        }
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

    /// Load connections.json by walking up from `search_dir`.
    pub fn load(search_dir: &Path) -> Result<Self> {
        let config_path = find_connections_json(search_dir);

        match config_path {
            Some(path) => {
                let content = std::fs::read_to_string(&path)?;
                let connections: HashMap<String, ConnectionConfig> =
                    serde_json::from_str(&content)?;
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
            anyhow::anyhow!("Connection '{}' not found in connections.json", name)
        })?;

        // Clone and substitute variables in all string fields
        let mut resolved = config.clone();
        resolved.host = resolved.host.map(|s| substitute_vars(&s, env_vars));
        resolved.password = resolved.password.map(|s| substitute_vars(&s, env_vars));
        resolved.user = resolved.user.map(|s| substitute_vars(&s, env_vars));
        resolved.database = resolved.database.map(|s| substitute_vars(&s, env_vars));
        resolved.path = resolved.path.map(|s| substitute_vars(&s, env_vars));

        Ok(resolved.to_url())
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

/// Find connections.json by walking up from search_dir.
fn find_connections_json(search_dir: &Path) -> Option<PathBuf> {
    let mut dir = search_dir.to_path_buf();
    loop {
        let candidate = dir.join("connections.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
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
            extra_params: HashMap::new(),
        };
        assert_eq!(
            config.to_url(),
            "postgres://admin:secret@localhost:5432/myapp"
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
            extra_params: HashMap::new(),
        };
        assert_eq!(config.to_url(), "sqlite:./data/app.db");
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
    fn test_find_connections_json() {
        // Create a temp directory structure
        let temp_dir = std::env::temp_dir().join("poste_test_connections");
        let sub_dir = temp_dir.join("sub").join("deep");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let config_path = temp_dir.join("connections.json");
        std::fs::write(
            &config_path,
            r#"{"test": {"dialect": "sqlite", "path": "test.db"}}"#,
        )
        .unwrap();

        // Search from deep subdirectory should find it
        let found = find_connections_json(&sub_dir);
        assert!(found.is_some());
        assert_eq!(found.unwrap(), config_path);

        // Cleanup
        std::fs::remove_dir_all(&temp_dir).ok();
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
            "sqlite:/home/user/db.sqlite"
        );
    }

    #[test]
    fn test_normalize_sqlite_relative_path() {
        assert_eq!(
            super::normalize_sqlite_connection("sqlite://./data.db").unwrap(),
            "sqlite:./data.db"
        );
        assert_eq!(
            super::normalize_sqlite_connection("sqlite://data.db").unwrap(),
            "sqlite:data.db"
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
            "sqlite:/absolute/path.db"
        );
        assert_eq!(
            super::normalize_sqlite_connection("./relative.db").unwrap(),
            "sqlite:./relative.db"
        );
    }

    #[test]
    fn test_normalize_sqlite_already_correct() {
        assert_eq!(
            super::normalize_sqlite_connection("sqlite:/path.db").unwrap(),
            "sqlite:/path.db"
        );
    }
}
