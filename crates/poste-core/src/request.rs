use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Protocol {
    Http,
    Redis,
    Mysql,
    Postgres,
    Mssql,
    ClickHouse,
    Sqlite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub name: Option<String>,
    pub protocol: Protocol,
    pub connection: String,
    /// Resolved body after file includes (`< filename`) and magic vars are expanded.
    /// Raw bytes — binary-safe for HTTP file uploads.
    pub body: Vec<u8>,
    /// Original body before file include resolution, for display in the request
    /// preview / Verbose tab.  If empty, falls back to `body` converted to string.
    pub raw_body: String,
}

impl Request {
    pub fn body_str(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }
}

/// Replace the database name in a connection URL.
/// "postgres://user:pass@host:5432/olddb" → "postgres://user:pass@host:5432/newdb"
/// Handles URLs with or without auth, port, and existing database. Any query
/// string / fragment (`?sslmode=require`) survives the swap — dropping it
/// silently disabled e.g. a pinned sslmode on the first `--database` switch.
pub fn replace_database_in_url(url: &str, new_db: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = &url[scheme_end + 3..];
        let (authority_path, suffix) = match after_scheme.find(['?', '#']) {
            Some(i) => (&after_scheme[..i], &after_scheme[i..]),
            None => (after_scheme, ""),
        };
        if let Some(last_slash) = authority_path.rfind('/') {
            let base = &url[..scheme_end + 3 + last_slash + 1];
            return format!("{}{}{}", base, new_db, suffix);
        }
        let prefix = &url[..scheme_end + 3];
        return format!("{}{}/{}{}", prefix, authority_path, new_db, suffix);
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::replace_database_in_url;

    #[test]
    fn replaces_database() {
        assert_eq!(
            replace_database_in_url("postgres://user:pass@host:5432/olddb", "newdb"),
            "postgres://user:pass@host:5432/newdb"
        );
        assert_eq!(
            replace_database_in_url("mysql://host/olddb", "newdb"),
            "mysql://host/newdb"
        );
    }

    #[test]
    fn appends_database_when_missing() {
        assert_eq!(
            replace_database_in_url("postgres://host:5432", "newdb"),
            "postgres://host:5432/newdb"
        );
    }

    #[test]
    fn keeps_query_string() {
        assert_eq!(
            replace_database_in_url("postgres://host:5432/olddb?sslmode=require", "newdb"),
            "postgres://host:5432/newdb?sslmode=require"
        );
        // no path db, query only
        assert_eq!(
            replace_database_in_url("postgres://host?sslmode=require", "newdb"),
            "postgres://host/newdb?sslmode=require"
        );
    }

    #[test]
    fn non_url_passthrough() {
        // sqlite has no `://` — the database override does not apply
        assert_eq!(
            replace_database_in_url("sqlite:/path/db.sqlite?mode=rwc", "newdb"),
            "sqlite:/path/db.sqlite?mode=rwc"
        );
    }
}
