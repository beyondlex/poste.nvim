use anyhow::Result;
use clap::Parser;

use poste_exec::sql_connection::{test_connection, ConnectionStore};

#[derive(Parser)]
pub enum ConnectionAction {
    /// List all connections from connections.json
    List {
        /// Directory to search for connections.json
        #[arg(long)]
        path: Option<String>,
        /// Environment name (for variable substitution)
        #[arg(short, long, default_value = "dev")]
        env: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Test a connection by name
    Test {
        /// Connection name
        name: String,
        /// Directory to search for connections.json
        #[arg(long)]
        path: Option<String>,
        /// Environment name (for variable substitution)
        #[arg(short, long, default_value = "dev")]
        env: String,
    },
}

pub async fn execute(action: ConnectionAction) -> Result<()> {
    match action {
        ConnectionAction::List { path, env, json } => {
            let search_dir = match path {
                Some(p) => std::path::PathBuf::from(p),
                None => std::env::current_dir()?,
            };

            let store = ConnectionStore::load(&search_dir)?;

            // Load env vars for variable substitution display
            let env_vars = crate::run::load_env_vars(&search_dir, &env);

            if json {
                let list = store.to_json_list();
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                if store.names().is_empty() {
                    println!("No connections found.");
                    if let Some(src) = store.source_path() {
                        println!("  Searched: {}", src.display());
                    }
                    return Ok(());
                }

                println!("Connections (from {:?}):\n", store.source_path());
                for item in store.to_json_list() {
                    let name = item["name"].as_str().unwrap_or("?");
                    let dialect = item["dialect"].as_str().unwrap_or("?");
                    let icon = match dialect {
                        "postgres" => "🐘",
                        "mysql" => "🐬",
                        "sqlite" => "📦",
                        _ => "❓",
                    };

                    if dialect == "sqlite" {
                        let path = item["path"].as_str().unwrap_or("?");
                        println!("  {} {} ({}) — {}", icon, name, dialect, path);
                    } else {
                        let host = item["host"].as_str().unwrap_or("?");
                        let port = item["port"].as_u64().unwrap_or(0);
                        let db = item["database"].as_str().unwrap_or("?");
                        println!(
                            "  {} {} ({}) — {}:{}/{}",
                            icon, name, dialect, host, port, db
                        );
                    }

                    // Show resolved URL
                    if let Ok(url) = store.resolve(name, &env_vars) {
                        println!("    → {}", url);
                    }
                }
            }
        }
        ConnectionAction::Test { name, path, env } => {
            let search_dir = match path {
                Some(p) => std::path::PathBuf::from(p),
                None => std::env::current_dir()?,
            };

            let store = ConnectionStore::load(&search_dir)?;
            let env_vars = crate::run::load_env_vars(&search_dir, &env);

            let config = store
                .get(&name)
                .ok_or_else(|| anyhow::anyhow!("Connection '{}' not found", name))?;

            // Resolve variables
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

            print!("Testing connection '{}' ... ", name);
            std::io::Write::flush(&mut std::io::stdout())?;

            match test_connection(&resolved).await {
                Ok(_) => println!("✓ OK"),
                Err(e) => {
                    println!("✗ FAILED");
                    eprintln!("  Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}
