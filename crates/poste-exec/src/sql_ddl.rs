//! DDL statement generation for PostgreSQL, MySQL, and SQLite.

use crate::sql_dialect::Dialect;

/// Column definition used when creating or adding columns.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub comment: Option<String>,
    pub extra: Option<String>,
}

/// Table schema used when generating CREATE TABLE.
#[derive(Debug, Clone)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub primary_key: Option<Vec<String>>,
    pub comment: Option<String>,
}

/// Trait for generating dialect-specific DDL statements.
pub trait DdlGenerator {
    fn create_table(&self, schema: &TableSchema) -> String;
    fn add_column(&self, table: &str, column: &ColumnDef) -> String;
    fn drop_column(&self, table: &str, column: &str) -> String;
    fn rename_column(&self, table: &str, old: &str, new: &str) -> String;
    fn alter_column_type(&self, table: &str, column: &str, new_type: &str) -> String;
    fn add_index(&self, table: &str, columns: &[&str], unique: bool) -> String;
    fn drop_table(&self, table: &str, cascade: bool) -> String;
}

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

fn column_def_sql(col: &ColumnDef, quote: &dyn Fn(&str) -> String) -> String {
    let mut s = format!("{} {}", quote(&col.name), col.col_type);
    if !col.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(ref d) = col.default {
        s.push_str(&format!(" DEFAULT {}", d));
    }
    s
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

pub struct PostgresDdl;

impl DdlGenerator for PostgresDdl {
    fn create_table(&self, schema: &TableSchema) -> String {
        use crate::sql_dialect::PostgresDialect;
        let d = PostgresDialect;
        let q = |name: &str| d.quote_identifier(name);

        let mut cols: Vec<String> = schema
            .columns
            .iter()
            .map(|c| format!("  {}", column_def_sql(c, &q)))
            .collect();

        if let Some(ref pk) = schema.primary_key {
            let pk_cols: Vec<String> = pk.iter().map(|c| q(c)).collect();
            cols.push(format!("  PRIMARY KEY ({})", pk_cols.join(", ")));
        }

        let mut result = format!(
            "CREATE TABLE {} (\n{}\n);",
            q(&schema.name),
            cols.join(",\n")
        );

        if let Some(ref comment) = schema.comment {
            if !comment.is_empty() {
                result.push_str(&format!(
                    "\n\nCOMMENT ON TABLE {} IS '{}';",
                    q(&schema.name),
                    comment.replace('\'', "''")
                ));
            }
        }

        for col in &schema.columns {
            if let Some(ref comment) = col.comment {
                if !comment.is_empty() {
                    result.push_str(&format!(
                        "\nCOMMENT ON COLUMN {}.{} IS '{}';",
                        q(&schema.name),
                        q(&col.name),
                        comment.replace('\'', "''")
                    ));
                }
            }
        }

        result
    }

    fn add_column(&self, table: &str, column: &ColumnDef) -> String {
        use crate::sql_dialect::PostgresDialect;
        let d = PostgresDialect;
        let q = |name: &str| d.quote_identifier(name);
        format!(
            "ALTER TABLE {} ADD COLUMN {};",
            q(table),
            column_def_sql(column, &q)
        )
    }

    fn drop_column(&self, table: &str, column: &str) -> String {
        use crate::sql_dialect::PostgresDialect;
        let d = PostgresDialect;
        let q = |name: &str| d.quote_identifier(name);
        format!("ALTER TABLE {} DROP COLUMN {};", q(table), q(column))
    }

    fn rename_column(&self, table: &str, old: &str, new: &str) -> String {
        use crate::sql_dialect::PostgresDialect;
        let d = PostgresDialect;
        let q = |name: &str| d.quote_identifier(name);
        format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {};",
            q(table),
            q(old),
            q(new)
        )
    }

    fn alter_column_type(&self, table: &str, column: &str, new_type: &str) -> String {
        use crate::sql_dialect::PostgresDialect;
        let d = PostgresDialect;
        let q = |name: &str| d.quote_identifier(name);
        format!(
            "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
            q(table),
            q(column),
            new_type
        )
    }

    fn add_index(&self, table: &str, columns: &[&str], unique: bool) -> String {
        use crate::sql_dialect::PostgresDialect;
        let d = PostgresDialect;
        let q = |name: &str| d.quote_identifier(name);
        let unique_kw = if unique { "UNIQUE " } else { "" };
        let col_list: Vec<String> = columns.iter().map(|c| q(c)).collect();
        let index_name = format!("idx_{}_{}", table, columns.join("_"));
        format!(
            "CREATE {}INDEX {} ON {} ({});",
            unique_kw,
            q(&index_name),
            q(table),
            col_list.join(", ")
        )
    }

    fn drop_table(&self, table: &str, cascade: bool) -> String {
        use crate::sql_dialect::PostgresDialect;
        let d = PostgresDialect;
        let q = |name: &str| d.quote_identifier(name);
        let suffix = if cascade { " CASCADE" } else { "" };
        format!("DROP TABLE {}{};", q(table), suffix)
    }
}

// ---------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------

pub struct MysqlDdl;

impl DdlGenerator for MysqlDdl {
    fn create_table(&self, schema: &TableSchema) -> String {
        use crate::sql_dialect::MysqlDialect;
        let d = MysqlDialect;
        let q = |name: &str| d.quote_identifier(name);

        let mut cols: Vec<String> = schema
            .columns
            .iter()
            .map(|c| {
                let mut s = format!("  {}", column_def_sql(c, &q));
                if let Some(ref extra) = c.extra {
                    if extra.to_lowercase().contains("auto_increment") {
                        s.push_str(" AUTO_INCREMENT");
                    }
                }
                if let Some(ref comment) = c.comment {
                    if !comment.is_empty() {
                        s.push_str(&format!(" COMMENT '{}'", comment.replace('\'', "''")));
                    }
                }
                s
            })
            .collect();

        if let Some(ref pk) = schema.primary_key {
            let pk_cols: Vec<String> = pk.iter().map(|c| q(c)).collect();
            cols.push(format!("  PRIMARY KEY ({})", pk_cols.join(", ")));
        }

        let mut result = format!(
            "CREATE TABLE {} (\n{}\n)",
            q(&schema.name),
            cols.join(",\n")
        );

        if let Some(ref comment) = schema.comment {
            if !comment.is_empty() {
                result.push_str(&format!(" COMMENT='{}'", comment.replace('\'', "''")));
            }
        }

        result.push(';');
        result
    }

    fn add_column(&self, table: &str, column: &ColumnDef) -> String {
        use crate::sql_dialect::MysqlDialect;
        let d = MysqlDialect;
        let q = |name: &str| d.quote_identifier(name);
        format!(
            "ALTER TABLE {} ADD COLUMN {};",
            q(table),
            column_def_sql(column, &q)
        )
    }

    fn drop_column(&self, table: &str, column: &str) -> String {
        use crate::sql_dialect::MysqlDialect;
        let d = MysqlDialect;
        let q = |name: &str| d.quote_identifier(name);
        format!("ALTER TABLE {} DROP COLUMN {};", q(table), q(column))
    }

    fn rename_column(&self, table: &str, old: &str, new_col: &str) -> String {
        use crate::sql_dialect::MysqlDialect;
        let d = MysqlDialect;
        let q = |name: &str| d.quote_identifier(name);
        // MySQL 8.0+: RENAME COLUMN; older: requires CHANGE COLUMN
        format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {};",
            q(table),
            q(old),
            q(new_col)
        )
    }

    fn alter_column_type(&self, table: &str, column: &str, new_type: &str) -> String {
        use crate::sql_dialect::MysqlDialect;
        let d = MysqlDialect;
        let q = |name: &str| d.quote_identifier(name);
        // MySQL uses MODIFY COLUMN; CHANGE COLUMN requires repeating the name
        format!(
            "ALTER TABLE {} MODIFY COLUMN {} {};",
            q(table),
            q(column),
            new_type
        )
    }

    fn add_index(&self, table: &str, columns: &[&str], unique: bool) -> String {
        use crate::sql_dialect::MysqlDialect;
        let d = MysqlDialect;
        let q = |name: &str| d.quote_identifier(name);
        let unique_kw = if unique { "UNIQUE " } else { "" };
        let col_list: Vec<String> = columns.iter().map(|c| q(c)).collect();
        let index_name = format!("idx_{}_{}", table, columns.join("_"));
        format!(
            "CREATE {}INDEX {} ON {} ({});",
            unique_kw,
            q(&index_name),
            q(table),
            col_list.join(", ")
        )
    }

    fn drop_table(&self, table: &str, _cascade: bool) -> String {
        use crate::sql_dialect::MysqlDialect;
        let d = MysqlDialect;
        let q = |name: &str| d.quote_identifier(name);
        // MySQL does not support CASCADE in DROP TABLE
        format!("DROP TABLE {};", q(table))
    }
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

pub struct SqliteDdl;

impl DdlGenerator for SqliteDdl {
    fn create_table(&self, schema: &TableSchema) -> String {
        use crate::sql_dialect::SqliteDialect;
        let d = SqliteDialect;
        let q = |name: &str| d.quote_identifier(name);

        let mut cols: Vec<String> = schema
            .columns
            .iter()
            .map(|c| format!("  {}", column_def_sql(c, &q)))
            .collect();

        if let Some(ref pk) = schema.primary_key {
            let pk_cols: Vec<String> = pk.iter().map(|c| q(c)).collect();
            cols.push(format!("  PRIMARY KEY ({})", pk_cols.join(", ")));
        }

        format!(
            "CREATE TABLE {} (\n{}\n);",
            q(&schema.name),
            cols.join(",\n")
        )
    }

    fn add_column(&self, table: &str, column: &ColumnDef) -> String {
        use crate::sql_dialect::SqliteDialect;
        let d = SqliteDialect;
        let q = |name: &str| d.quote_identifier(name);
        // SQLite only supports ADD COLUMN (not DROP/RENAME in older versions)
        format!(
            "ALTER TABLE {} ADD COLUMN {};",
            q(table),
            column_def_sql(column, &q)
        )
    }

    fn drop_column(&self, table: &str, column: &str) -> String {
        use crate::sql_dialect::SqliteDialect;
        let d = SqliteDialect;
        let q = |name: &str| d.quote_identifier(name);
        // Requires SQLite 3.35.0+
        format!("ALTER TABLE {} DROP COLUMN {};", q(table), q(column))
    }

    fn rename_column(&self, table: &str, old: &str, new: &str) -> String {
        use crate::sql_dialect::SqliteDialect;
        let d = SqliteDialect;
        let q = |name: &str| d.quote_identifier(name);
        // Requires SQLite 3.25.0+
        format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {};",
            q(table),
            q(old),
            q(new)
        )
    }

    fn alter_column_type(&self, table: &str, _column: &str, _new_type: &str) -> String {
        use crate::sql_dialect::SqliteDialect;
        let d = SqliteDialect;
        let q = |name: &str| d.quote_identifier(name);
        // SQLite does not support ALTER COLUMN TYPE.
        // Standard workaround: recreate the table.
        format!(
            "-- SQLite does not support ALTER COLUMN TYPE directly.\n\
             -- Recreate the table to change column types:\n\
             -- CREATE TABLE {}_new (...);\n\
             -- INSERT INTO {}_new SELECT * FROM {};\n\
             -- DROP TABLE {};\n\
             -- ALTER TABLE {}_new RENAME TO {};",
            q(table),
            q(table),
            q(table),
            q(table),
            q(table),
            q(table)
        )
    }

    fn add_index(&self, table: &str, columns: &[&str], unique: bool) -> String {
        use crate::sql_dialect::SqliteDialect;
        let d = SqliteDialect;
        let q = |name: &str| d.quote_identifier(name);
        let unique_kw = if unique { "UNIQUE " } else { "" };
        let col_list: Vec<String> = columns.iter().map(|c| q(c)).collect();
        let index_name = format!("idx_{}_{}", table, columns.join("_"));
        format!(
            "CREATE {}INDEX {} ON {} ({});",
            unique_kw,
            q(&index_name),
            q(table),
            col_list.join(", ")
        )
    }

    fn drop_table(&self, table: &str, _cascade: bool) -> String {
        use crate::sql_dialect::SqliteDialect;
        let d = SqliteDialect;
        let q = |name: &str| d.quote_identifier(name);
        // SQLite does not support CASCADE
        format!("DROP TABLE {};", q(table))
    }
}

/// Get a DdlGenerator for the given dialect name.
pub fn ddl_for(dialect: &str) -> Option<Box<dyn DdlGenerator>> {
    match dialect {
        "postgres" => Some(Box::new(PostgresDdl)),
        "mysql" => Some(Box::new(MysqlDdl)),
        "sqlite" => Some(Box::new(SqliteDdl)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- PostgreSQL ---

    #[test]
    fn test_pg_create_table() {
        let ddl = PostgresDdl;
        let schema = TableSchema {
            name: "users".to_string(),
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    col_type: "SERIAL".to_string(),
                    nullable: false,
                    default: None,
                    comment: None,
                    extra: None,
                },
                ColumnDef {
                    name: "name".to_string(),
                    col_type: "VARCHAR(255)".to_string(),
                    nullable: false,
                    default: None,
                    comment: None,
                    extra: None,
                },
            ],
            primary_key: Some(vec!["id".to_string()]),
            comment: None,
        };
        let sql = ddl.create_table(&schema);
        assert!(sql.contains("CREATE TABLE \"users\""));
        assert!(sql.contains("\"id\" SERIAL NOT NULL"));
        assert!(sql.contains("\"name\" VARCHAR(255) NOT NULL"));
        assert!(sql.contains("PRIMARY KEY (\"id\")"));
    }

    #[test]
    fn test_pg_add_column() {
        let ddl = PostgresDdl;
        let col = ColumnDef {
            name: "email".to_string(),
            col_type: "TEXT".to_string(),
            nullable: true,
            default: None,
            comment: None,
            extra: None,
        };
        let sql = ddl.add_column("users", &col);
        assert_eq!(sql, "ALTER TABLE \"users\" ADD COLUMN \"email\" TEXT;");
    }

    #[test]
    fn test_pg_drop_column() {
        let ddl = PostgresDdl;
        let sql = ddl.drop_column("users", "email");
        assert_eq!(sql, "ALTER TABLE \"users\" DROP COLUMN \"email\";");
    }

    #[test]
    fn test_pg_rename_column() {
        let ddl = PostgresDdl;
        let sql = ddl.rename_column("users", "name", "full_name");
        assert_eq!(
            sql,
            "ALTER TABLE \"users\" RENAME COLUMN \"name\" TO \"full_name\";"
        );
    }

    #[test]
    fn test_pg_alter_column_type() {
        let ddl = PostgresDdl;
        let sql = ddl.alter_column_type("users", "age", "BIGINT");
        assert_eq!(
            sql,
            "ALTER TABLE \"users\" ALTER COLUMN \"age\" TYPE BIGINT;"
        );
    }

    #[test]
    fn test_pg_add_index() {
        let ddl = PostgresDdl;
        let sql = ddl.add_index("users", &["email"], true);
        assert!(sql.contains("CREATE UNIQUE INDEX"));
        assert!(sql.contains("ON \"users\""));
        assert!(sql.contains("\"email\""));
    }

    #[test]
    fn test_pg_drop_table_cascade() {
        let ddl = PostgresDdl;
        assert_eq!(
            ddl.drop_table("users", true),
            "DROP TABLE \"users\" CASCADE;"
        );
        assert_eq!(ddl.drop_table("users", false), "DROP TABLE \"users\";");
    }

    // --- MySQL ---

    #[test]
    fn test_mysql_create_table() {
        let ddl = MysqlDdl;
        let schema = TableSchema {
            name: "orders".to_string(),
            columns: vec![ColumnDef {
                name: "id".to_string(),
                col_type: "INT".to_string(),
                nullable: false,
                default: None,
                comment: None,
                extra: Some("auto_increment".to_string()),
            }],
            primary_key: Some(vec!["id".to_string()]),
            comment: None,
        };
        let sql = ddl.create_table(&schema);
        assert!(sql.contains("CREATE TABLE `orders`"));
        assert!(sql.contains("`id` INT NOT NULL AUTO_INCREMENT"));
        assert!(sql.contains("PRIMARY KEY (`id`)"));
    }

    #[test]
    fn test_mysql_add_column() {
        let ddl = MysqlDdl;
        let col = ColumnDef {
            name: "status".to_string(),
            col_type: "VARCHAR(50)".to_string(),
            nullable: false,
            default: Some("'active'".to_string()),
            comment: None,
            extra: None,
        };
        let sql = ddl.add_column("orders", &col);
        assert!(sql.contains("ALTER TABLE `orders` ADD COLUMN"));
        assert!(sql.contains("`status` VARCHAR(50) NOT NULL DEFAULT 'active'"));
    }

    #[test]
    fn test_mysql_alter_column_type() {
        let ddl = MysqlDdl;
        let sql = ddl.alter_column_type("orders", "amount", "DECIMAL(10,2)");
        assert_eq!(
            sql,
            "ALTER TABLE `orders` MODIFY COLUMN `amount` DECIMAL(10,2);"
        );
    }

    #[test]
    fn test_mysql_drop_table_no_cascade() {
        let ddl = MysqlDdl;
        // MySQL ignores cascade parameter
        assert_eq!(ddl.drop_table("orders", true), "DROP TABLE `orders`;");
    }

    // --- SQLite ---

    #[test]
    fn test_sqlite_add_column() {
        let ddl = SqliteDdl;
        let col = ColumnDef {
            name: "score".to_string(),
            col_type: "REAL".to_string(),
            nullable: true,
            default: Some("0.0".to_string()),
            comment: None,
            extra: None,
        };
        let sql = ddl.add_column("players", &col);
        assert!(sql.contains("ALTER TABLE \"players\" ADD COLUMN"));
        assert!(sql.contains("\"score\" REAL DEFAULT 0.0"));
    }

    #[test]
    fn test_sqlite_alter_column_type_comment() {
        let ddl = SqliteDdl;
        let sql = ddl.alter_column_type("t", "col", "TEXT");
        assert!(sql.contains("does not support"));
        assert!(sql.contains("Recreate"));
    }

    #[test]
    fn test_sqlite_rename_column() {
        let ddl = SqliteDdl;
        let sql = ddl.rename_column("t", "old_col", "new_col");
        assert_eq!(
            sql,
            "ALTER TABLE \"t\" RENAME COLUMN \"old_col\" TO \"new_col\";"
        );
    }

    #[test]
    fn test_ddl_for() {
        assert!(ddl_for("postgres").is_some());
        assert!(ddl_for("mysql").is_some());
        assert!(ddl_for("sqlite").is_some());
        assert!(ddl_for("oracle").is_none());
    }
}
