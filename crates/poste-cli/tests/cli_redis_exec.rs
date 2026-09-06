// Process-level integration test for `poste redis-exec`.
// Only runs when REDIS_TEST_URL is provided (CI service / local docker).

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn redis_exec_emits_event_stream() {
    let Ok(url) = std::env::var("REDIS_TEST_URL") else {
        return;
    };
    let bin = env!("CARGO_BIN_EXE_poste");

    let mut child = Command::new(bin)
        .args(["redis-exec"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn poste");

    let payload = serde_json::json!({
        "connection": url,
        "commands": [
            ["PING"],
            ["SET", "poste:cli:test", "v1", "EX", "60"],
            ["GET", "poste:cli:test"],
            ["NOSUCHCMD", "zzz"],
        ]
    });
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();

    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is JSON"))
        .collect();

    // 4 × (progress + result) + 1 summary
    assert_eq!(lines.len(), 9, "got: {lines:?}");

    assert_eq!(lines[0]["type"], "progress");
    assert_eq!(lines[0]["total"], 4);
    assert_eq!(lines[1]["type"], "result");
    assert_eq!(lines[1]["status"], "PONG");

    // error keeps seq order, does not stop the batch
    // (lines: 0-7 are progress/result pairs, line 7 = 4th command's result)
    let err_ev = &lines[7];
    assert_eq!(err_ev["status"], "error");
    assert_eq!(err_ev["seq"], 4);
    assert!(err_ev["error"].as_str().unwrap().contains("NOSUCHCMD"));

    let summary = &lines[8];
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["total"], 4);
    assert_eq!(summary["failed"], 1);

    // cleanup test key
    let _ = Command::new(bin)
        .args(["redis-exec"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .and_then(|mut c| {
            c.stdin.take().unwrap().write_all(
                serde_json::json!({
                    "connection": url,
                    "commands": [["DEL", "poste:cli:test"]]
                })
                .to_string()
                .as_bytes(),
            )?;
            c.wait()
        });
}

#[test]
fn redis_exec_rejects_non_redis_urls() {
    let bin = env!("CARGO_BIN_EXE_poste");
    let mut child = Command::new(bin)
        .args(["redis-exec"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"connection":"postgres://x/y","commands":[["PING"]]}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Not a redis connection URL"),
        "stderr: {stderr}"
    );
}

#[test]
fn redis_exec_rejects_malformed_stdin() {
    let bin = env!("CARGO_BIN_EXE_poste");
    let mut child = Command::new(bin)
        .args(["redis-exec"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"not json").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
}
