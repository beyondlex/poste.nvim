/// Check if a protocol is SQL-based.
pub fn is_sql_protocol(protocol: &poste_core::Protocol) -> bool {
    matches!(
        protocol,
        poste_core::Protocol::Postgres | poste_core::Protocol::Mysql | poste_core::Protocol::Sqlite
    )
}

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
