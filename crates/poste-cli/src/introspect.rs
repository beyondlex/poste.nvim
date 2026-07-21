use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
pub struct IntrospectArgs {
    /// Connection name (from connections.json)
    pub name: String,
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
    /// Directory to search for connections.json
    #[arg(long)]
    pub path: Option<String>,
    /// Environment name (for variable substitution)
    #[arg(short, long, default_value = "dev")]
    pub env: String,
}

pub async fn execute(args: IntrospectArgs) -> Result<()> {
    use poste_exec::sql_connection::ConnectionStore;
    use poste_exec::sql_introspect::{self, IntrospectParams, IntrospectType};

    let search_dir = match args.path {
        Some(p) => std::path::PathBuf::from(p),
        None => std::env::current_dir()?,
    };

    // Load and resolve connection
    let store = ConnectionStore::load(&search_dir)?;
    let env_vars = crate::run::load_env_vars(&search_dir, &args.env);

    let config = store
        .get(&args.name)
        .ok_or_else(|| anyhow::anyhow!("Connection '{}' not found", args.name))?;

    // Resolve variables and build URL
    let mut resolved = config.clone();
    resolved.host = resolved
        .host
        .map(|s| poste_core::substitute_vars(&s, &env_vars));
    resolved.password = resolved
        .password
        .map(|s| poste_core::substitute_vars(&s, &env_vars));
    resolved.user = resolved
        .user
        .map(|s| poste_core::substitute_vars(&s, &env_vars));
    resolved.database = resolved
        .database
        .map(|s| poste_core::substitute_vars(&s, &env_vars));
    resolved.path = resolved
        .path
        .map(|s| poste_core::substitute_vars(&s, &env_vars));

    let mut connection_url = resolved.to_url();
    let dialect_name = resolved.dialect.clone();

    // Override database if --database flag is provided
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
