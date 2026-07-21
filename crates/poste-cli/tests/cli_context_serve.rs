use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn test_serve_detect() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_poste"))
        .args(["context", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn poste serve");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let req = json!({
        "id": 1,
        "method": "detect",
        "params": {
            "sql": "SELECT * FROM users WHERE ",
            "offset": 26,
            "dialect": "generic"
        }
    });

    writeln!(stdin, "{}", req).unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();

    assert_eq!(resp["id"], 1);
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["result"]["ctx_type"], "column");
    assert!(resp["result"]["tables"].is_array());

    // Send EOF
    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn test_serve_stmt() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_poste"))
        .args(["context", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn poste serve");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let req = json!({
        "id": 2,
        "method": "stmt",
        "params": {
            "sql": "SELECT * FROM users;\nSELECT * FROM posts;",
            "cursor_line": 0
        }
    });

    writeln!(stdin, "{}", req).unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();

    assert_eq!(resp["id"], 2);
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["result"]["start_line"], 0);

    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn test_serve_unknown_method() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_poste"))
        .args(["context", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn poste serve");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let req = json!({
        "id": 3,
        "method": "bogus",
        "params": {}
    });

    writeln!(stdin, "{}", req).unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();

    assert_eq!(resp["id"], 3);
    assert_eq!(resp["ok"], false);
    assert!(resp["error"].as_str().unwrap().contains("bogus"));

    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn test_serve_invalid_params() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_poste"))
        .args(["context", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn poste serve");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Missing required params (no offset)
    let req = json!({
        "id": 4,
        "method": "detect",
        "params": {
            "sql": "SELECT 1"
        }
    });

    writeln!(stdin, "{}", req).unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();

    assert_eq!(resp["id"], 4);
    assert_eq!(resp["ok"], false);
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("invalid detect params"));

    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn test_serve_eof_clean_exit() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_poste"))
        .args(["context", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn poste serve");

    // Close stdin to send EOF
    child.stdin.take().unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
}
