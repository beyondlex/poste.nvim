use anyhow::Result;
use clap::{Parser, Subcommand};

mod connection;
mod context;
mod exec_file;
mod introspect;
mod util;

#[derive(Parser)]
#[command(name = "poste")]
#[command(about = "Execute requests from files")]
#[command(disable_version_flag = true)]
#[command(subcommand_required = false)]
#[command(arg_required_else_help = true)]
struct Cli {
    #[arg(short = 'v', long = "version", help = "Print version information")]
    version: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage SQL connections
    Connection {
        #[command(subcommand)]
        action: connection::ConnectionAction,
    },
    /// Introspect database structure (list databases, schemas, tables, columns, indexes)
    Introspect(introspect::IntrospectArgs),
    /// SQL context detection (for completion/indicator placement)
    Context {
        #[command(subcommand)]
        action: context::ContextAction,
    },
    /// Execute a SQL file with streaming progress
    ExecFile(exec_file::ExecFileArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.version {
        let tag = env!("POSTE_TAG");
        let date = env!("POSTE_BUILD_DATE");
        if date == "unknown" {
            println!("poste {tag}");
        } else {
            println!("poste {tag} ({date})");
        }
        return Ok(());
    }

    match cli.command {
        Some(Commands::Connection { action }) => {
            connection::execute(action).await?;
        }
        Some(Commands::Introspect(args)) => {
            introspect::execute(args).await?;
        }
        Some(Commands::Context { action }) => {
            context::execute(action)?;
        }
        Some(Commands::ExecFile(args)) => {
            exec_file::execute(args).await?;
        }
        None => {}
    }

    Ok(())
}
