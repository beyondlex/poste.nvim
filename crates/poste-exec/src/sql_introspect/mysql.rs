use anyhow::Result;
use serde_json::{json, Value};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::Row;

use super::{IntrospectParams, IntrospectType};
use crate::sql_ddl;
use crate::sql_dialect::{Dialect, MysqlDialect};

pub(super) async fn introspect_mysql(params: &IntrospectParams) -> Result<Value> {
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&params.connection_url)
        .await?;

    let dialect = MysqlDialect;

    fn col(row: &MySqlRow, name: &str) -> String {
        let bytes: Vec<u8> = row.get(name);
        String::from_utf8_lossy(&bytes).into_owned()
    }
    fn col_opt(row: &MySqlRow, name: &str) -> Option<String> {
        let bytes: Option<Vec<u8>> = row.get(name);
        bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
    }
    fn col_idx(row: &MySqlRow, idx: usize) -> String {
        let bytes: Vec<u8> = row.get(idx);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    let items: Vec<Value> = match params.introspect_type {
        IntrospectType::Databases => {
            let sql = dialect.list_databases();
            let rows = sqlx::query(sql).fetch_all(&pool).await?;
            rows.iter()
                .map(|row| json!({ "name": col_idx(row, 0) }))
                .collect()
        }
        IntrospectType::Schemas => Vec::new(),
        IntrospectType::Tables => {
            let sql = dialect.list_tables();
            let rows = sqlx::query(sql).fetch_all(&pool).await?;
            rows.iter()
                .map(|row| {
                    json!({
                        "name": col_idx(row, 0),
                        "type": "BASE TABLE",
                    })
                })
                .collect()
        }
        IntrospectType::Columns => {
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for columns introspection")
            })?;
            let sql = dialect.list_columns().replace("{}", table);
            let rows = sqlx::query(&sql).fetch_all(&pool).await?;
            let fk_sql = format!(
                "SELECT kcu.COLUMN_NAME AS col, kcu.REFERENCED_TABLE_NAME AS ref_table, \
                        kcu.REFERENCED_COLUMN_NAME AS ref_col \
                 FROM information_schema.key_column_usage kcu \
                 WHERE kcu.table_schema = DATABASE() AND kcu.table_name = '{}' \
                   AND kcu.referenced_table_name IS NOT NULL",
                table.replace('\'', "''")
            );
            let fk_rows = sqlx::query(&fk_sql)
                .fetch_all(&pool)
                .await
                .unwrap_or_default();
            let fk_map: std::collections::HashMap<String, (String, String)> = fk_rows
                .iter()
                .map(|r| (col(r, "col"), (col(r, "ref_table"), col(r, "ref_col"))))
                .collect();
            rows.iter()
                .map(|row| {
                    let col_name = col(row, "Field");
                    let (ref_table, ref_col) = fk_map.get(&col_name).cloned().unwrap_or_default();
                    json!({
                        "name": col_name,
                        "type": col(row, "Type"),
                        "nullable": col(row, "Null") == "YES",
                        "default": col_opt(row, "Default"),
                        "key": col(row, "Key"),
                        "extra": col(row, "Extra"),
                        "comment": col_opt(row, "Comment"),
                        "fk_table": ref_table,
                        "fk_column": ref_col,
                    })
                })
                .collect()
        }
        IntrospectType::Indexes => {
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for indexes introspection")
            })?;
            let sql = dialect.list_indexes().replace("{}", table);
            let rows = sqlx::query(&sql).fetch_all(&pool).await?;
            use std::collections::BTreeMap;
            let mut index_map: BTreeMap<String, (Vec<String>, bool)> = BTreeMap::new();
            for row in &rows {
                let key_name = col(row, "Key_name");
                let column_name = col(row, "Column_name");
                let is_unique = row.try_get::<i32, _>("Non_unique").unwrap_or(1) == 0;
                let entry = index_map.entry(key_name).or_default();
                entry.0.push(column_name);
                if is_unique {
                    entry.1 = true;
                }
            }
            index_map
                .into_iter()
                .map(|(name, (columns, unique))| {
                    json!({
                        "name": name,
                        "columns": columns,
                        "unique": unique,
                        "definition": format!("INDEX {} ({})", name, columns.join(", ")),
                    })
                })
                .collect()
        }
        IntrospectType::Ddl => {
            build_create_table_from_introspect_mysql(&pool, params.table.as_deref()).await?
        }
    };

    pool.close().await;

    Ok(json!({
        "type": "introspect",
        "introspect_type": params.introspect_type.as_str(),
        "items": items,
        "schema": params.schema,
        "table": params.table,
        "dialect": "mysql",
    }))
}

async fn build_create_table_from_introspect_mysql(
    pool: &MySqlPool,
    table: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    use sqlx::Row;

    let table =
        table.ok_or_else(|| anyhow::anyhow!("table parameter required for ddl introspection"))?;

    fn col(row: &MySqlRow, name: &str) -> String {
        let bytes: Vec<u8> = row.get(name);
        String::from_utf8_lossy(&bytes).into_owned()
    }
    fn col_opt(row: &MySqlRow, name: &str) -> Option<String> {
        let bytes: Option<Vec<u8>> = row.get(name);
        bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    let col_sql = format!("SHOW FULL COLUMNS FROM `{}`", table);
    let col_rows = sqlx::query(&col_sql).fetch_all(pool).await?;

    let mut pk_cols: Vec<String> = Vec::new();
    let mut columns: Vec<sql_ddl::ColumnDef> = Vec::new();

    fn mysql_default_needs_quoting(col_type: &str) -> bool {
        let lower = col_type.to_lowercase();
        lower.starts_with("char")
            || lower.starts_with("varchar")
            || lower.starts_with("text")
            || lower.starts_with("tinytext")
            || lower.starts_with("mediumtext")
            || lower.starts_with("longtext")
            || lower.starts_with("enum")
            || lower.starts_with("set")
    }

    for row in &col_rows {
        let name = col(row, "Field");
        let col_type = col(row, "Type");
        let nullable = col(row, "Null") == "YES";
        let mut default = col_opt(row, "Default");
        let key = col(row, "Key");
        let comment = col_opt(row, "Comment");
        let extra = col_opt(row, "Extra");

        if let Some(ref d) = default {
            if mysql_default_needs_quoting(&col_type) && !d.starts_with('\'') {
                default = Some(format!("'{}'", d.replace('\'', "''")));
            }
        }

        if key == "PRI" {
            pk_cols.push(name.clone());
        }

        columns.push(sql_ddl::ColumnDef {
            name,
            col_type,
            nullable,
            default,
            comment,
            extra,
        });
    }

    let table_comment_sql = format!(
        "SELECT TABLE_COMMENT FROM information_schema.TABLES \
         WHERE TABLE_NAME = '{}' AND TABLE_SCHEMA = DATABASE()",
        table.replace('\'', "''")
    );
    let table_comment: Option<String> = sqlx::query_scalar(&table_comment_sql)
        .fetch_optional(pool)
        .await?
        .filter(|s: &String| !s.is_empty());

    let schema_def = sql_ddl::TableSchema {
        name: table.to_string(),
        columns,
        primary_key: if pk_cols.is_empty() {
            None
        } else {
            Some(pk_cols)
        },
        comment: table_comment,
    };

    if let Some(ddl_generator) = sql_ddl::ddl_for("mysql") {
        let ddl = ddl_generator.create_table(&schema_def);
        return Ok(vec![
            json!({"ddl": ddl, "type": "ddl", "table": table, "dialect": "mysql"}),
        ]);
    }

    Ok(vec![
        json!({"ddl": format!("-- Could not create DDL for table '{}'", table)}),
    ])
}
