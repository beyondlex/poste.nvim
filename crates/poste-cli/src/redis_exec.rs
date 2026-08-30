use anyhow::Result;
use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;

/// Execute pre-tokenized redis commands read from stdin as a single JSON
/// line: {"connection": "redis://...", "commands": [["GET","key"], ...]}.
/// Lua is the only parser (poste-redis EXECUTION-PLAN D1) — this binary
/// never sees `.redis` file text. Emits one JSON event per line:
/// progress → result (per command) → summary.
#[derive(Parser)]
pub struct RedisExecArgs {
    /// Max elements emitted per array/map reply (extra marked truncated)
    #[arg(long, default_value_t = 1000)]
    pub max_items: u64,
    /// Max bytes per string reply (extra marked truncated)
    #[arg(long, default_value_t = 65536)]
    pub max_bytes: u64,
}

#[derive(Deserialize)]
struct RedisExecRequest {
    connection: String,
    commands: Vec<Vec<String>>,
}

pub async fn execute(args: RedisExecArgs) -> Result<()> {
    let mut input = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut tokio::io::stdin(), &mut input).await?;
    let req: RedisExecRequest = serde_json::from_str(input.trim())
        .map_err(|e| anyhow::anyhow!("Invalid stdin JSON: {}", e))?;

    let total = req.commands.len();
    let started = Instant::now();
    let mut failed = 0u64;

    poste_exec::redis_executor::execute_commands_with(
        &req.connection,
        &req.commands,
        args.max_items as usize,
        args.max_bytes as usize,
        |outcome| {
            // Line-buffered stdout: the Lua side reads newline-delimited JSON
            println!(
                "{}",
                json!({
                    "type": "progress",
                    "index": outcome.seq,
                    "total": total,
                    "command": outcome.command,
                })
            );
            let mut ev = json!({
                "type": "result",
                "seq": outcome.seq,
                "command": outcome.command,
                "status": outcome.status,
                "latency_ms": outcome.latency_ms,
                "value": outcome.value,
            });
            if let Some(err) = &outcome.error {
                failed += 1;
                ev["error"] = json!(err);
                ev["status"] = json!("error");
            }
            println!("{}", ev);
        },
    )
    .await?;

    println!(
        "{}",
        json!({
            "type": "summary",
            "total": total,
            "failed": failed,
            "elapsed_ms": started.elapsed().as_millis(),
        })
    );
    Ok(())
}
