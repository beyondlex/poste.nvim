use anyhow::Result;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

use super::{IntrospectParams, IntrospectType};
use crate::sql_ddl;
use crate::sql_dialect::{Dialect, PostgresDialect};

pub(super) async fn introspect_postgres(params: &IntrospectParams) -> Result<Value> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&params.connection_url)
        .await?;

    let dialect = PostgresDialect;

    let items: Vec<Value> = match params.introspect_type {
        IntrospectType::Databases => {
            let sql = dialect.list_databases();
            let rows = sqlx::query(sql).fetch_all(&pool).await?;
            rows.iter()
                .map(|row| json!({ "name": row.get::<String, _>("datname") }))
                .collect()
        }
        IntrospectType::Schemas => {
            let sql = dialect.list_schemas().unwrap();
            let rows = sqlx::query(sql).fetch_all(&pool).await?;
            rows.iter()
                .map(|row| json!({ "name": row.get::<String, _>("schema_name") }))
                .collect()
        }
        IntrospectType::Tables => {
            let schema = params.schema.as_deref().unwrap_or("public");
            let sql = dialect.list_tables();
            let rows = sqlx::query(sql).bind(schema).fetch_all(&pool).await?;
            rows.iter()
                .map(|row| {
                    json!({
                        "name": row.get::<String, _>("table_name"),
                        "type": row.get::<String, _>("table_type"),
                    })
                })
                .collect()
        }
        IntrospectType::Columns => {
            let schema = params.schema.as_deref().unwrap_or("public");
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for columns introspection")
            })?;
            let sql = dialect.list_columns();
            let rows = sqlx::query(sql)
                .bind(schema)
                .bind(table)
                .fetch_all(&pool)
                .await?;
            let fk_rows = sqlx::query(
                "SELECT kcu.column_name, ccu.table_name AS fk_table, ccu.column_name AS fk_column \
                 FROM information_schema.table_constraints tc \
                 JOIN information_schema.key_column_usage kcu \
                   ON tc.constraint_name = kcu.constraint_name \
                   AND tc.table_schema = kcu.table_schema \
                 JOIN information_schema.constraint_column_usage ccu \
                   ON ccu.constraint_name = tc.constraint_name \
                   AND ccu.table_schema = tc.table_schema \
                 WHERE tc.constraint_type = 'FOREIGN KEY' \
                   AND tc.table_schema = $1 AND tc.table_name = $2",
            )
            .bind(schema)
            .bind(table)
            .fetch_all(&pool)
            .await
            .unwrap_or_default();
            #[derive(Default)]
            struct FkInfo {
                table: String,
                column: String,
            }
            let fk_map: std::collections::HashMap<String, FkInfo> = fk_rows
                .iter()
                .map(|r| {
                    (
                        r.get::<String, _>("column_name"),
                        FkInfo {
                            table: r.get::<String, _>("fk_table"),
                            column: r.get::<String, _>("fk_column"),
                        },
                    )
                })
                .collect();
            rows.iter()
                .map(|row| {
                    let col_name: String = row.get("column_name");
                    let char_max_len: Option<i32> = row.get("character_maximum_length");
                    let fk = fk_map.get(&col_name);
                    json!({
                        "name": col_name,
                        "type": row.get::<String, _>("data_type"),
                        "nullable": row.get::<String, _>("is_nullable") == "YES",
                        "default": row.get::<Option<String>, _>("column_default"),
                        "max_length": char_max_len,
                        "comment": row.get::<Option<String>, _>("comment"),
                        "fk_table": fk.map(|f| f.table.as_str()),
                        "fk_column": fk.map(|f| f.column.as_str()),
                    })
                })
                .collect()
        }
        IntrospectType::Indexes => {
            let schema = params.schema.as_deref().unwrap_or("public");
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for indexes introspection")
            })?;
            let sql = dialect.list_indexes();
            let rows = sqlx::query(sql)
                .bind(schema)
                .bind(table)
                .fetch_all(&pool)
                .await?;
            rows.iter()
                .map(|row| {
                    let is_unique: bool = row.get("is_unique");
                    let columns: String = row.get("columns");
                    json!({
                        "name": row.get::<String, _>("indexname"),
                        "definition": row.get::<String, _>("indexdef"),
                        "unique": is_unique,
                        "columns": columns.split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>(),
                    })
                })
                .collect()
        }
        IntrospectType::Ddl => {
            let schema = params.schema.as_deref().unwrap_or("public");
            let table = params
                .table
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("table parameter required for ddl introspection"))?;
            build_create_table_from_introspect_postgres(&pool, schema, table).await?
        }
    };

    pool.close().await;

    Ok(json!({
        "type": "introspect",
        "introspect_type": params.introspect_type.as_str(),
        "items": items,
        "schema": params.schema,
        "table": params.table,
        "dialect": "postgres",
    }))
}

async fn build_create_table_from_introspect_postgres(
    pool: &sqlx::PgPool,
    schema: &str,
    table: &str,
) -> Result<Vec<serde_json::Value>> {
    use sqlx::Row;

    let dialect = PostgresDialect;

    let col_sql = dialect.list_columns();
    let col_rows = sqlx::query(col_sql)
        .bind(schema)
        .bind(table)
        .fetch_all(pool)
        .await?;

    let mut pk_cols: Vec<String> = Vec::new();
    let mut columns: Vec<sql_ddl::ColumnDef> = Vec::new();

    for row in &col_rows {
        let name: String = row.get("column_name");
        let data_type: String = row.get("data_type");
        let is_nullable: String = row.get("is_nullable");
        let default: Option<String> = row.get("column_default");

        columns.push(sql_ddl::ColumnDef {
            name,
            col_type: data_type,
            nullable: is_nullable == "YES",
            default,
            comment: None,
            extra: None,
        });
    }

    let pk_sql = r#"SELECT kc.column_name
       FROM information_schema.table_constraints tc
       JOIN information_schema.key_column_usage kc
         ON kc.constraint_name = tc.constraint_name
        AND kc.table_schema = tc.table_schema
       WHERE tc.constraint_type = 'PRIMARY KEY'
         AND tc.table_schema = $1
         AND tc.table_name = $2
       ORDER BY kc.ordinal_position"#
        .to_string();
    let pk_rows = sqlx::query(&pk_sql)
        .bind(schema)
        .bind(table)
        .fetch_all(pool)
        .await?;
    for row in &pk_rows {
        pk_cols.push(row.get::<String, _>("column_name"));
    }

    let comment_sql = r#"SELECT a.attname AS column_name, pgd.description
FROM pg_catalog.pg_class pc
JOIN pg_catalog.pg_attribute a ON a.attrelid = pc.oid
LEFT JOIN pg_catalog.pg_description pgd ON pgd.objoid = pc.oid AND pgd.objsubid = a.attnum
WHERE pc.relname = $1
  AND pc.relnamespace = (SELECT oid FROM pg_catalog.pg_namespace WHERE nspname = $2)
  AND a.attnum > 0
  AND NOT a.attisdropped"#;
    let comment_rows = sqlx::query(comment_sql)
        .bind(table)
        .bind(schema)
        .fetch_all(pool)
        .await?;
    let col_comments: std::collections::HashMap<String, String> = comment_rows
        .iter()
        .filter_map(|row| {
            let col_name: String = row.get("column_name");
            let desc: Option<String> = row.get("description");
            desc.filter(|d| !d.is_empty()).map(|d| (col_name, d))
        })
        .collect();

    for col in &mut columns {
        if let Some(comment) = col_comments.get(&col.name) {
            col.comment = Some(comment.clone());
        }
    }

    let table_comment_sql = r#"SELECT pgd.description
FROM pg_catalog.pg_class pc
LEFT JOIN pg_catalog.pg_description pgd ON pgd.objoid = pc.oid AND pgd.objsubid = 0
WHERE pc.relname = $1
  AND pc.relnamespace = (SELECT oid FROM pg_catalog.pg_namespace WHERE nspname = $2)"#;
    let table_comment: Option<String> = sqlx::query(table_comment_sql)
        .bind(table)
        .bind(schema)
        .fetch_optional(pool)
        .await?
        .and_then(|row| row.get("description"))
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

    if let Some(ddl_generator) = sql_ddl::ddl_for("postgres") {
        let ddl = ddl_generator.create_table(&schema_def);
        return Ok(vec![
            json!({"ddl": ddl, "type": "ddl", "table": table, "dialect": "postgres"}),
        ]);
    }

    Ok(vec![
        json!({"ddl": format!("-- Could not create DDL for table '{}'", table)}),
    ])
}
