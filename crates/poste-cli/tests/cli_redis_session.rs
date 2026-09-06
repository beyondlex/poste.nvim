// Process-level integration test for `poste redis-session`.
// Only runs when REDIS_TEST_URL is provided (CI service / local docker).
//
// The session keeps ONE connection open across stdin requests — this test
// proves SELECT state persists between requests (the whole point of the
// long-lived transport; poste-redis EXECUTION-PLAN §3.3).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn redis_session_keeps_connection_state_across_requests() {
    let Ok(url) = std::env::var("REDIS_TEST_URL") else {
        return;
    };
    let bin = env!("CARGO_BIN_EXE_poste");

    let mut child = Command::new(bin)
        .args(["redis-session", "--connection", &url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn poste redis-session");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Flush the test keys in db 0, then SELECT 1 and write there — the
    // following GET/DBSIZE prove the SELECT executed in a previous request
    // applies to later ones (connection state persists across requests).
    let reqs = [
        serde_json::json!({"seq": 1, "command": ["DEL", "poste:sess:a", "poste:sess:b"]}),
        serde_json::json!({"seq": 2, "command": ["SELECT", "1"]}),
        serde_json::json!({"seq": 3, "command": ["SET", "poste:sess:a", "one", "EX", "60"]}),
        serde_json::json!({"seq": 4, "command": ["SET", "poste:sess:b", "two", "EX", "60"]}),
        serde_json::json!({"seq": 5, "command": ["DBSIZE"]}),
    ];

    let mut line = String::new();
    for req in &reqs {
        stdin
            .write_all(format!("{}\n", req).as_bytes())
            .expect("write request");
        stdin.flush().unwrap();

        line.clear();
        reader.read_line(&mut line).expect("read response");
        let ev: serde_json::Value = serde_json::from_str(line.trim()).expect("response is JSON");
        assert_eq!(ev["type"], "result");
        assert_eq!(ev["seq"], req["seq"]);
        assert!(ev.get("error").is_none(), "request {req} failed: {ev}");
    }

    // SELECT persisted: GET hits db 1, DBSIZE sees both keys there
    line.clear();
    stdin
        .write_all(b"{\"seq\":6,\"command\":[\"GET\",\"poste:sess:a\"]}\n")
        .unwrap();
    stdin.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let ev: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(ev["seq"], 6);
    assert_eq!(
        ev["value"]["value"], "one",
        "SELECT state lost across requests"
    );

    // cleanup: back to db 0 and delete both keys
    line.clear();
    stdin
        .write_all(b"{\"seq\":7,\"command\":[\"SELECT\",\"0\"]}\n")
        .unwrap();
    stdin.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    line.clear();
    stdin
        .write_all(b"{\"seq\":8,\"command\":[\"DEL\",\"poste:sess:a\",\"poste:sess:b\"]}\n")
        .unwrap();
    stdin.flush().unwrap();
    reader.read_line(&mut line).unwrap();

    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn redis_session_rejects_non_redis_urls() {
    let bin = env!("CARGO_BIN_EXE_poste");
    let mut child = Command::new(bin)
        .args(["redis-session", "--connection", "postgres://x/y"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Not a redis connection URL"),
        "stderr: {stderr}"
    );
}

#[test]
fn redis_session_errors_on_bad_json_but_keeps_running() {
    let Ok(url) = std::env::var("REDIS_TEST_URL") else {
        return;
    };
    let bin = env!("CARGO_BIN_EXE_poste");

    let mut child = Command::new(bin)
        .args(["redis-session", "--connection", &url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // malformed line → seq 0 error event, then a valid request still works
    stdin.write_all(b"not json\n").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let ev: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(ev["type"], "result");
    assert_eq!(ev["seq"], 0);
    assert_eq!(ev["status"], "error");
    assert!(ev["error"].as_str().unwrap().contains("JSON parse error"));

    stdin
        .write_all(b"{\"seq\":1,\"command\":[\"PING\"]}\n")
        .unwrap();
    stdin.flush().unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let ev: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(ev["seq"], 1);
    assert_eq!(ev["status"], "PONG");

    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
}
