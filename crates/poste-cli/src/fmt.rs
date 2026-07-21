use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
pub struct FmtArgs {
    /// Files to format (default: stdin)
    #[arg()]
    pub files: Vec<String>,
    /// Check formatting without modifying (exit 1 if unformatted)
    #[arg(long)]
    pub check: bool,
    /// Read from stdin
    #[arg(long)]
    pub stdin: bool,
    /// Modify files in-place (default behavior without --check)
    #[arg(short, long)]
    pub in_place: bool,
}

pub fn execute(args: FmtArgs) -> Result<()> {
    use poste_core::Formatter;
    use std::io::Read;

    let use_stdin = args.stdin || args.files.is_empty();

    if use_stdin {
        let mut content = String::new();
        std::io::stdin().read_to_string(&mut content)?;
        let formatted = Formatter::format(&content);
        if args.check {
            if content != formatted {
                std::process::exit(1);
            }
        } else {
            print!("{}", formatted);
        }
        return Ok(());
    }

    for path in &args.files {
        let original = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path, e))?;
        let formatted = Formatter::format(&original);

        if args.check {
            if original != formatted {
                eprintln!("{}: unformatted", path);
                std::process::exit(1);
            }
        } else if args.in_place || !args.check {
            std::fs::write(path, &formatted)
                .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path, e))?;
        } else {
            print!("{}", formatted);
        }
    }

    Ok(())
}
