use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Protocol {
    Http,
    Redis,
    Mysql,
    Postgres,
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
/// Handles URLs with or without auth, port, and existing database.
pub fn replace_database_in_url(url: &str, new_db: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = &url[scheme_end + 3..];
        if let Some(last_slash) = after_scheme.rfind('/') {
            let base = &url[..scheme_end + 3 + last_slash + 1];
            return format!("{}{}", base, new_db);
        }
        return format!("{}/{}", url, new_db);
    }
    url.to_string()
}
