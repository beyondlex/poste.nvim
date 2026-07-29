use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
pub struct IntrospectArgs {
    /// Connection URL (Lua-resolved, not a name from connections.json)
    #[arg(long)]
    pub connection_url: String,
    /// Introspection type: databases, schemas, tables, columns, indexes
    #[arg(long)]
    pub r#type: String,
    /// Schema name (for PG tables/columns/indexes)
    #[arg(long)]
    pub schema: Option<String>,
    /// Table name (for columns/indexes)
    #[arg(long)]
    pub table: Option<String>,
    /// Database name (overrides connection's default database)
    #[arg(long)]
    pub database: Option<String>,
}

pub async fn execute(args: IntrospectArgs) -> Result<()> {
    use poste_exec::sql_introspect::{self, IntrospectParams, IntrospectType};

    let mut connection_url = args.connection_url;
    let dialect_name = if connection_url.starts_with("sqlite:") {
        "sqlite".to_string()
    } else if connection_url.starts_with("mysql://") {
        "mysql".to_string()
    } else {
        "postgres".to_string()
    };

    if let Some(ref db) = args.database {
        connection_url = poste_core::replace_database_in_url(&connection_url, db);
    }

    let params = IntrospectParams {
        connection_url,
        dialect_name,
        introspect_type: IntrospectType::parse_str(&args.r#type)?,
        schema: args.schema,
        table: args.table,
    };

    let result = sql_introspect::introspect(&params).await?;
    println!("{}", serde_json::to_string(&result)?);

    Ok(())
}
