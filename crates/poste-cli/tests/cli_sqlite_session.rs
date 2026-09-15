// Process-level integration test for `poste session` (NDJSON loop).
//
// Regression: a per-statement SQL error used to `?`-propagate out of the
// sqlite/postgres/mysql session loops and kill the whole session process —
// one typo'd statement dropped the connection. Statement failures must come
// back as `status:"error"` result events while the loop keeps serving (the
// mssql/clickhouse paths already worked that way; docs/schema.md specifies
// the error-event contract). Runs against a local sqlite file — no server.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

fn write_req(stdin: &mut std::process::ChildStdin, req: &serde_json::Value) {
    stdin
        .write_all(format!("{}\n", req).as_bytes())
        .expect("write request");
    stdin.flush().unwrap();
}

fn next_event(rx: &Receiver<String>, timeout: Duration) -> serde_json::Value {
    match rx.recv_timeout(timeout) {
        Ok(line) => serde_json::from_str(line.trim()).expect("each session line is JSON"),
        Err(_) => panic!("timeout waiting for session event"),
    }
}

fn wait_result(rx: &Receiver<String>, seq: u64) -> serde_json::Value {
    let ev = next_event(rx, Duration::from_secs(10));
    assert_eq!(ev["type"], "result", "expected result, got: {ev}");
    assert_eq!(ev["seq"].as_u64(), Some(seq), "wrong seq, got: {ev}");
    ev
}

#[test]
fn sqlite_session_survives_a_bad_statement() {
    let bin = env!("CARGO_BIN_EXE_poste");
    let db = std::env::temp_dir().join(format!("poste_session_test_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let url = format!("sqlite:{}", db.display());

    let mut child = Command::new(bin)
        .args(["session", "--connection", &url, "--timeout", "10"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn poste session");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut seq = 0u64;

    // Setup: table + one row.
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "sql": "CREATE TABLE t (id INTEGER)"}),
    );
    assert_eq!(wait_result(&rx, seq)["status"], "ok");
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "sql": "INSERT INTO t VALUES (42)"}),
    );
    let ev = wait_result(&rx, seq);
    assert_eq!(ev["status"], "ok");
    assert_eq!(ev["affected_rows"].as_u64(), Some(1));

    // A bad statement must come back as an error EVENT…
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "sql": "SELECT * FROM no_such_table"}),
    );
    let ev = wait_result(&rx, seq);
    assert_eq!(ev["status"], "error", "expected an error event: {ev}");
    assert!(
        ev["error"].as_str().is_some_and(|e| !e.is_empty()),
        "error event carries a message: {ev}"
    );

    // …and the session must still be alive and serving afterwards.
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "sql": "SELECT id FROM t"}),
    );
    let ev = wait_result(&rx, seq);
    assert_eq!(
        ev["status"], "ok",
        "session died after an error event: {ev}"
    );
    assert_eq!(ev["row_count"].as_u64(), Some(1));
    assert_eq!(ev["rows"][0][0], 42);

    // A DML failure survives the same way.
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "sql": "INSERT INTO missing_t VALUES (1)"}),
    );
    assert_eq!(wait_result(&rx, seq)["status"], "error");
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "sql": "SELECT COUNT(*) FROM t"}),
    );
    assert_eq!(wait_result(&rx, seq)["status"], "ok");

    drop(stdin);
    while rx.recv().is_ok() {}
    let status = child.wait().unwrap();
    assert!(status.success(), "session exited non-zero");

    let _ = std::fs::remove_file(&db);
}
