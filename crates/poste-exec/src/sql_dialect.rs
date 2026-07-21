//! SQL dialect abstraction for PostgreSQL, MySQL, and SQLite.
//!
//! Each database has different SQL syntax for introspection queries,
//! identifier quoting, and schema support. This module encapsulates
//! those differences behind a common `Dialect` trait.

use poste_core::Protocol;

/// Abstraction over database-specific SQL syntax and introspection queries.
pub trait Dialect: Send + Sync {
    /// Dialect name: "postgres" | "mysql" | "sqlite"
    fn name(&self) -> &str;

    /// SQL to list all databases (or equivalent top-level containers).
    fn list_databases(&self) -> &str;

    /// SQL to list schemas within a database. `None` if not supported.
    /// PostgreSQL supports schemas; MySQL and SQLite do not.
    fn list_schemas(&self) -> Option<&str>;

    /// SQL to list tables in a given database/schema.
    /// The caller should bind parameters as appropriate for each dialect.
    fn list_tables(&self) -> &str;

    /// SQL to list columns of a given table.
    fn list_columns(&self) -> &str;

    /// SQL to list indexes of a given table.
    fn list_indexes(&self) -> &str;

    /// SQL to describe a table's full structure (columns, types, constraints).
    fn describe_table(&self) -> &str;

    /// Whether this dialect supports the schema hierarchy level
    /// (database → schema → table).
    fn supports_schema(&self) -> bool;

    /// Quote an identifier (table name, column name, etc.) for safe use in SQL.
    fn quote_identifier(&self, name: &str) -> String;

    /// Default port for this database's network protocol.
    fn default_port(&self) -> u16;

    /// Map a raw column type string to a display-friendly type name.
    fn type_mapping<'a>(&self, col_type: &'a str) -> &'a str;
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

pub struct PostgresDialect;

impl Dialect for PostgresDialect {
    fn name(&self) -> &str {
        "postgres"
    }

    fn list_databases(&self) -> &str {
        "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname"
    }

    fn list_schemas(&self) -> Option<&str> {
        Some(
            "SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name NOT IN ('pg_catalog', 'information_schema') \
             ORDER BY schema_name",
        )
    }

    fn list_tables(&self) -> &str {
        "SELECT table_name, table_type FROM information_schema.tables \
         WHERE table_schema = $1 ORDER BY table_name"
    }

    fn list_columns(&self) -> &str {
        "SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, \
         c.character_maximum_length, \
         pgd.description AS comment \
         FROM information_schema.columns c \
         LEFT JOIN pg_catalog.pg_description pgd \
           ON pgd.objoid = (SELECT oid FROM pg_catalog.pg_class \
                             WHERE relname = c.table_name \
                             AND relnamespace = (SELECT oid FROM pg_catalog.pg_namespace \
                                                 WHERE nspname = c.table_schema)) \
           AND pgd.objsubid = c.ordinal_position::int \
         WHERE c.table_schema = $1 AND c.table_name = $2 \
         ORDER BY c.ordinal_position"
    }

    fn list_indexes(&self) -> &str {
        "SELECT i.indexname, i.indexdef, \
         ix.indisunique AS is_unique, \
         COALESCE(a.columns, '') AS columns \
         FROM pg_indexes i \
         JOIN pg_class c ON c.relname = i.indexname \
           AND c.relnamespace = (SELECT oid FROM pg_namespace WHERE nspname = i.schemaname) \
         JOIN pg_index ix ON ix.indexrelid = c.oid \
         LEFT JOIN LATERAL ( \
           SELECT string_agg(a.attname, ',' ORDER BY u.ord) AS columns \
           FROM unnest(ix.indkey) WITH ORDINALITY u(key, ord) \
           JOIN pg_attribute a ON a.attrelid = ix.indrelid AND a.attnum = u.key \
           WHERE a.attnum > 0 \
         ) a ON true \
         WHERE i.schemaname = $1 AND i.tablename = $2 \
         ORDER BY i.indexname"
    }

    fn describe_table(&self) -> &str {
        "SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, \
         c.character_maximum_length, \
         CASE WHEN pk.column_name IS NOT NULL THEN true ELSE false END AS is_primary_key \
         FROM information_schema.columns c \
         LEFT JOIN ( \
           SELECT kcu.column_name \
           FROM information_schema.table_constraints tc \
           JOIN information_schema.key_column_usage kcu \
             ON tc.constraint_name = kcu.constraint_name \
             AND tc.table_schema = kcu.table_schema \
           WHERE tc.constraint_type = 'PRIMARY KEY' \
             AND tc.table_schema = $1 AND tc.table_name = $2 \
         ) pk ON c.column_name = pk.column_name \
         WHERE c.table_schema = $1 AND c.table_name = $2 \
         ORDER BY c.ordinal_position"
    }

    fn supports_schema(&self) -> bool {
        true
    }

    fn quote_identifier(&self, name: &str) -> String {
        // PostgreSQL uses double quotes: "name"
        format!("\"{}\"", name.replace('"', "\"\""))
    }

    fn default_port(&self) -> u16 {
        5432
    }

    fn type_mapping<'a>(&self, col_type: &'a str) -> &'a str {
        match col_type {
            "integer" | "int4" => "integer",
            "bigint" | "int8" => "bigint",
            "smallint" | "int2" => "smallint",
            "text" => "text",
            "character varying" | "varchar" => "varchar",
            "character" | "char" => "char",
            "boolean" | "bool" => "boolean",
            "timestamp without time zone" | "timestamp" => "timestamp",
            "timestamp with time zone" | "timestamptz" => "timestamptz",
            "date" => "date",
            "time without time zone" | "time" => "time",
            "numeric" | "decimal" => "numeric",
            "real" | "float4" => "real",
            "double precision" | "float8" => "double",
            "json" => "json",
            "jsonb" => "jsonb",
            "uuid" => "uuid",
            "bytea" => "bytea",
            _ => col_type,
        }
    }
}

// ---------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------

pub struct MysqlDialect;

impl Dialect for MysqlDialect {
    fn name(&self) -> &str {
        "mysql"
    }

    fn list_databases(&self) -> &str {
        "SHOW DATABASES"
    }

    fn list_schemas(&self) -> Option<&str> {
        // MySQL doesn't have a separate schema concept; database IS the schema.
        None
    }

    fn list_tables(&self) -> &str {
        "SHOW TABLES"
    }

    fn list_columns(&self) -> &str {
        "SHOW FULL COLUMNS FROM `{}`"
    }

    fn list_indexes(&self) -> &str {
        "SHOW INDEX FROM `{}`"
    }

    fn describe_table(&self) -> &str {
        "DESCRIBE `{}`"
    }

    fn supports_schema(&self) -> bool {
        false
    }

    fn quote_identifier(&self, name: &str) -> String {
        // MySQL uses backticks: `name`
        format!("`{}`", name.replace('`', "``"))
    }

    fn default_port(&self) -> u16 {
        3306
    }

    fn type_mapping<'a>(&self, col_type: &'a str) -> &'a str {
        let lower = col_type.to_lowercase();
        if lower.starts_with("int") {
            "integer"
        } else if lower.starts_with("bigint") {
            "bigint"
        } else if lower.starts_with("smallint") {
            "smallint"
        } else if lower.starts_with("tinyint(1)") {
            "boolean"
        } else if lower.starts_with("tinyint") {
            "tinyint"
        } else if lower.starts_with("varchar") {
            "varchar"
        } else if lower.starts_with("char") {
            "char"
        } else if lower.starts_with("text") || lower.contains("text") {
            "text"
        } else if lower.starts_with("datetime") {
            "datetime"
        } else if lower.starts_with("timestamp") {
            "timestamp"
        } else if lower.starts_with("date") {
            "date"
        } else if lower.starts_with("decimal") || lower.starts_with("numeric") {
            "decimal"
        } else if lower.starts_with("float") {
            "float"
        } else if lower.starts_with("double") {
            "double"
        } else if lower.starts_with("json") {
            "json"
        } else if lower.starts_with("blob") || lower.contains("blob") {
            "blob"
        } else {
            col_type
        }
    }
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

pub struct SqliteDialect;

impl Dialect for SqliteDialect {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn list_databases(&self) -> &str {
        // SQLite has a single "main" database (plus attached ones).
        // PRAGMA database_list shows attached databases.
        "PRAGMA database_list"
    }

    fn list_schemas(&self) -> Option<&str> {
        // SQLite doesn't have schemas.
        None
    }

    fn list_tables(&self) -> &str {
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    }

    fn list_columns(&self) -> &str {
        "PRAGMA table_info({})"
    }

    fn list_indexes(&self) -> &str {
        "PRAGMA index_list({})"
    }

    fn describe_table(&self) -> &str {
        "PRAGMA table_info({})"
    }

    fn supports_schema(&self) -> bool {
        false
    }

    fn quote_identifier(&self, name: &str) -> String {
        // SQLite supports double quotes for identifiers
        format!("\"{}\"", name.replace('"', "\"\""))
    }

    fn default_port(&self) -> u16 {
        0 // SQLite is file-based, no network port
    }

    fn type_mapping<'a>(&self, col_type: &'a str) -> &'a str {
        let upper = col_type.to_uppercase();
        if upper.contains("INT") {
            "integer"
        } else if upper.contains("TEXT") || upper.contains("CLOB") {
            "text"
        } else if upper.contains("BLOB") || col_type.is_empty() {
            "blob"
        } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
            "real"
        } else if upper.contains("CHAR") || upper.contains("VARCHAR") {
            "text"
        } else if upper.contains("BOOL") {
            "boolean"
        } else if upper.contains("DATE") || upper.contains("TIME") {
            "text" // SQLite stores dates as text/integer
        } else {
            col_type
        }
    }
}

/// Get the appropriate Dialect for a given protocol.
/// Returns `None` for non-SQL protocols.
pub fn dialect_for(protocol: &Protocol) -> Option<Box<dyn Dialect>> {
    match protocol {
        Protocol::Postgres => Some(Box::new(PostgresDialect)),
        Protocol::Mysql => Some(Box::new(MysqlDialect)),
        Protocol::Sqlite => Some(Box::new(SqliteDialect)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgres_dialect() {
        let d = PostgresDialect;
        assert_eq!(d.name(), "postgres");
        assert!(d.supports_schema());
        assert_eq!(d.default_port(), 5432);
        assert_eq!(d.quote_identifier("users"), "\"users\"");
        assert_eq!(d.quote_identifier("my\"table"), "\"my\"\"table\"");
        assert!(d.list_schemas().is_some());
        assert!(d.list_tables().contains("information_schema"));
        assert_eq!(d.type_mapping("integer"), "integer");
        assert_eq!(d.type_mapping("character varying"), "varchar");
    }

    #[test]
    fn test_mysql_dialect() {
        let d = MysqlDialect;
        assert_eq!(d.name(), "mysql");
        assert!(!d.supports_schema());
        assert_eq!(d.default_port(), 3306);
        assert_eq!(d.quote_identifier("users"), "`users`");
        assert_eq!(d.quote_identifier("my`table"), "`my``table`");
        assert!(d.list_schemas().is_none());
        assert_eq!(d.list_tables(), "SHOW TABLES");
        assert_eq!(d.type_mapping("tinyint(1)"), "boolean");
        assert_eq!(d.type_mapping("varchar(255)"), "varchar");
    }

    #[test]
    fn test_sqlite_dialect() {
        let d = SqliteDialect;
        assert_eq!(d.name(), "sqlite");
        assert!(!d.supports_schema());
        assert_eq!(d.default_port(), 0);
        assert_eq!(d.quote_identifier("users"), "\"users\"");
        assert!(d.list_schemas().is_none());
        assert!(d.list_tables().contains("sqlite_master"));
        assert_eq!(d.type_mapping("INTEGER"), "integer");
        assert_eq!(d.type_mapping("TEXT"), "text");
        assert_eq!(d.type_mapping("BLOB"), "blob");
        assert_eq!(d.type_mapping(""), "blob");
    }

    #[test]
    fn test_dialect_for() {
        assert!(dialect_for(&Protocol::Postgres).is_some());
        assert!(dialect_for(&Protocol::Mysql).is_some());
        assert!(dialect_for(&Protocol::Sqlite).is_some());
        assert!(dialect_for(&Protocol::Http).is_none());
        assert!(dialect_for(&Protocol::Redis).is_none());
    }
}
