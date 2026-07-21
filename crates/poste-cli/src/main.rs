use anyhow::Result;
use clap::{Parser, Subcommand};

mod connection;
mod context;
mod fmt;
mod import;
mod introspect;
mod resolve;
mod run;
mod serve;
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
    /// Execute a request at a specific line
    Run(run::RunArgs),
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
    /// Format .http/.rest files
    Fmt(fmt::FmtArgs),
    /// Import specs from external tools (OpenAPI, Swagger, Postman)
    Import {
        #[command(subcommand)]
        source: import::ImportSource,
    },
    /// Resolve variables in a request block
    Resolve(resolve::ResolveArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.version {
        println!("poste {}", env!("POSTE_TAG"));
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
        Some(Commands::Run(args)) => {
            run::execute(args).await?;
        }
        Some(Commands::Fmt(args)) => {
            fmt::execute(args)?;
        }
        Some(Commands::Import { source }) => {
            import::execute(source)?;
        }
        Some(Commands::Resolve(args)) => {
            resolve::execute(args)?;
        }
        None => {}
    }

    Ok(())
}
