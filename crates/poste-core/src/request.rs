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

impl Protocol {
    /// Sniff a SQL protocol from a connection URL's scheme, first match
    /// winning. The single Rust copy of this rule (the CLI's exec/session/
    /// introspect entry points all use it) and the mirror of Lua's
    /// `constants.URL_SCHEMES` in poste-db, which lists the same prefixes.
    ///
    /// `mariadb://` belongs here: sqlx's MySQL driver declares
    /// `URL_SCHEMES = ["mysql", "mariadb"]`, so a raw `url = "mariadb://…"`
    /// connection is connectable, and a sniff that ignored the scheme failed
    /// it as "cannot determine protocol" while the Lua side already read it
    /// as MySQL for display and session context.
    pub fn from_sql_url(url: &str) -> Option<Self> {
        if url.starts_with("sqlite:") {
            Some(Self::Sqlite)
        } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Some(Self::Postgres)
        } else if url.starts_with("mysql://") || url.starts_with("mariadb://") {
            Some(Self::Mysql)
        } else if url.starts_with("mssql://") {
            Some(Self::Mssql)
        } else if url.starts_with("clickhouse://") {
            Some(Self::ClickHouse)
        } else {
            None
        }
    }
}

/// Mask the password portion of a connection URL for safe display.
/// `postgres://user:secret@host/db` → `postgres://user:****@host/db`.
///
/// Every message that quotes a resolved connection URL goes through this —
/// the URL carries the real password, and a CLI error line ends up in a
/// terminal, in `:messages`, and in saved result files. The scan is confined
/// to the authority (up to the first `/`, `?` or `#`): a database name may
/// legally contain `@`, and treating that as userinfo would both hide the
/// wrong segment and mangle the host.
pub fn mask_url_password(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let Some(at) = rest[..authority_end].rfind('@') else {
        return url.to_string();
    };
    let userinfo = &rest[..at];
    let Some(colon) = userinfo.rfind(':') else {
        return url.to_string();
    };
    format!(
        "{}{}:****{}",
        &url[..scheme_end + 3],
        &userinfo[..colon],
        &rest[at..]
    )
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
    use super::{mask_url_password, replace_database_in_url, Protocol};

    #[test]
    fn sniffs_every_supported_scheme() {
        // The mirror pair is Lua's constants.URL_SCHEMES — a scheme added
        // there without being added here makes a legal connections.toml
        // `url = "…"` fail only on execution.
        assert_eq!(
            Protocol::from_sql_url("sqlite:./app.db"),
            Some(Protocol::Sqlite)
        );
        assert_eq!(
            Protocol::from_sql_url("postgres://h/db"),
            Some(Protocol::Postgres)
        );
        assert_eq!(
            Protocol::from_sql_url("postgresql://h/db"),
            Some(Protocol::Postgres)
        );
        assert_eq!(
            Protocol::from_sql_url("mysql://h/db"),
            Some(Protocol::Mysql)
        );
        assert_eq!(
            Protocol::from_sql_url("mariadb://user:pw@h:3306/db"),
            Some(Protocol::Mysql),
            "sqlx's MySQL driver lists mariadb:// as a valid scheme"
        );
        assert_eq!(
            Protocol::from_sql_url("mssql://h/db"),
            Some(Protocol::Mssql)
        );
        assert_eq!(
            Protocol::from_sql_url("clickhouse://h/db"),
            Some(Protocol::ClickHouse)
        );
    }

    #[test]
    fn rejects_foreign_and_bare_urls() {
        assert_eq!(Protocol::from_sql_url("redis://h:6379/0"), None);
        assert_eq!(Protocol::from_sql_url("http://h/db"), None);
        assert_eq!(Protocol::from_sql_url(""), None);
        // scheme sniffing is prefix-based and case-sensitive, like Lua's
        assert_eq!(Protocol::from_sql_url("PostgreSQL://h/db"), None);
    }

    #[test]
    fn masks_password_but_keeps_rest() {
        assert_eq!(
            mask_url_password("postgres://alice:secret@db.example.com:5432/myapp"),
            "postgres://alice:****@db.example.com:5432/myapp"
        );
        // an empty user with only a password is redis' usual form
        assert_eq!(
            mask_url_password("redis://:s3cr3t@cache.internal:6379/0"),
            "redis://:****@cache.internal:6379/0"
        );
        // percent-encoded still masks (the raw value never reaches display)
        assert_eq!(
            mask_url_password("postgres://alice:p%40ss@db.example.com/myapp"),
            "postgres://alice:****@db.example.com/myapp"
        );
    }

    #[test]
    fn leaves_urls_without_password_unchanged() {
        assert_eq!(
            mask_url_password("postgres://alice@db.example.com:5432/myapp"),
            "postgres://alice@db.example.com:5432/myapp"
        );
        assert_eq!(
            mask_url_password("sqlite:./data/app.db?mode=rwc"),
            "sqlite:./data/app.db?mode=rwc"
        );
    }

    #[test]
    fn ignores_an_at_that_is_not_in_the_authority() {
        // `@` is legal in a database name; treating it as userinfo separator
        // hid nothing and mangled the host instead.
        assert_eq!(
            mask_url_password("postgres://db.example.com:5432/team@billing"),
            "postgres://db.example.com:5432/team@billing"
        );
        // same through a query string
        assert_eq!(
            mask_url_password("postgres://db.example.com/team@x?sslmode=require"),
            "postgres://db.example.com/team@x?sslmode=require"
        );
        // ... but a real password ahead of it still masks
        assert_eq!(
            mask_url_password("postgres://alice:secret@db.example.com/team@billing"),
            "postgres://alice:****@db.example.com/team@billing"
        );
    }

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
