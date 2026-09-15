// Golden NDJSON tests for `poste exec-file`.
//
// These pin the exec-file wire format (progress/result/summary events) so
// the per-dialect execution code can be refactored with a byte-verifiable
// safety net. Volatile fields (timings, the temp db path) are normalized
// before comparison; everything else — including key ORDER is irrelevant but
// field presence, values and event sequence must match the goldens exactly.
//
// Regenerate with: UPDATE_GOLDENS=1 cargo test --test exec_file_golden

use serde_json::Value;

use std::process::{Command, Stdio};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/exec_file");

/// Normalize one event line: canonical JSON (BTreeMap keys), timings zeroed,
/// the temp sqlite path replaced with a stable placeholder.
fn normalize(line: &str, tmpdir: &str) -> String {
    let mut ev: Value = serde_json::from_str(line).expect("each stdout line is JSON");
    scrub(&mut ev, tmpdir);
    serde_json::to_string(&ev).expect("canonical serialize")
}

fn scrub(ev: &mut Value, tmpdir: &str) {
    match ev {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                match k.as_str() {
                    "execution_time_ms" | "total_time_ms" | "latency_ms" => {
                        *v = Value::Number(0.into());
                    }
                    // the sqlite file stem rides along as the database name
                    "database" => {
                        if let Some(s) = v.as_str() {
                            *v = Value::String(s.replace(tmpdir, "<TMP>"));
                        }
                    }
                    _ => scrub(v, tmpdir),
                }
            }
            // the connection string carries the temp db path
            if let Some(conn) = map.get_mut("connection") {
                if let Some(s) = conn.as_str() {
                    *conn = Value::String(s.replace(tmpdir, "<TMP>"));
                }
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                scrub(v, tmpdir);
            }
        }
        _ => {}
    }
}

fn run_case(case: &str, mode: &str, sql: &str) -> Vec<String> {
    let tmp = std::env::temp_dir().join(format!("poste_golden_{case}.sqlite"));
    let _ = std::fs::remove_file(&tmp);

    let sql_path = std::env::temp_dir().join(format!("poste_golden_{case}.sql"));
    std::fs::write(&sql_path, sql).expect("write sql file");

    let url = format!("sqlite:{}", tmp.display());
    let output = Command::new(env!("CARGO_BIN_EXE_poste"))
        .args([
            "exec-file",
            sql_path.to_str().unwrap(),
            "--json",
            "--mode",
            mode,
            "--connection",
            &url,
        ])
        .stdin(Stdio::null())
        .output()
        .expect("spawn poste exec-file");
    assert!(
        output.status.success(),
        "greedy exec-file exits 0 (per-statement errors are events): {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tmpdir = tmp.parent().unwrap().to_str().unwrap().to_string();
    let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| normalize(l, &tmpdir))
        .collect();

    std::fs::remove_file(&sql_path).ok();
    std::fs::remove_file(&tmp).ok();
    lines
}

fn compare_or_update(case: &str, actual: &[String]) {
    let golden_path = format!("{FIXTURES}/{case}.ndjson");
    let golden_text = std::fs::read_to_string(&golden_path).ok();

    if std::env::var("UPDATE_GOLDENS").is_ok() || golden_text.is_none() {
        std::fs::create_dir_all(FIXTURES).expect("create fixtures dir");
        std::fs::write(&golden_path, actual.join("\n") + "\n").expect("write golden");
        return;
    }
    let golden_lines: Vec<String> = golden_text
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect();

    for (i, (g, a)) in golden_lines.iter().zip(actual.iter()).enumerate() {
        assert_eq!(g, a, "event {i} diverged from golden {case}");
    }
    assert_eq!(
        golden_lines.len(),
        actual.len(),
        "event count diverged from golden {case}"
    );
}

const MIXED_SQL: &str = r#"
-- a plain comment line never reaches the event stream
-- @connection sqlite://this-directive-is-ignored-when---connection-is-passed
CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, note TEXT);

INSERT INTO users (name, score, note) VALUES
  ('alice', 9.5, NULL),
  ('bob', 7.0, 'has ; semicolon');

SELECT id, name, score, note FROM users ORDER BY id;

UPDATE users SET score = 10.0 WHERE id = 1;

SELECT count(*) AS n FROM users;

DELETE FROM users WHERE id = 2;

SELECT * FROM missing_table;

USE blog;

SELECT 'unterminated;
"#;

#[test]
fn golden_greedy_mixed() {
    let actual = run_case("greedy_mixed", "greedy", MIXED_SQL);
    compare_or_update("greedy_mixed", &actual);
}

const TXN_SQL: &str = r#"
CREATE TABLE ledger (id INTEGER PRIMARY KEY, amount INTEGER);
INSERT INTO ledger (amount) VALUES (100);
UPDATE ledger SET amount = amount - 10 WHERE id = 1;
INSERT INTO ledger (amount) VALUES ('not a number');
INSERT INTO ledger (amount) VALUES (999);
"#;

#[test]
fn golden_transaction_rollback() {
    // transaction mode: the failing INSERT aborts with ROLLBACK and the run
    // stops there — the trailing INSERT must never execute.
    let actual = run_case("transaction_rollback", "transaction", TXN_SQL);
    compare_or_update("transaction_rollback", &actual);
}

#[test]
fn golden_max_rows_truncation() {
    let sql = r#"
CREATE TABLE nums (n INTEGER);
INSERT INTO nums VALUES (1),(2),(3),(4),(5),(6),(7),(8);
SELECT n FROM nums ORDER BY n;
"#;
    // max_rows is a CLI flag, not part of the sql: run with an explicit cap
    let tmp = std::env::temp_dir().join("poste_golden_mr.sqlite");
    let _ = std::fs::remove_file(&tmp);
    let sql_path = std::env::temp_dir().join("poste_golden_max_rows.sql");
    std::fs::write(&sql_path, sql).expect("write sql");

    let url = format!("sqlite:{}", tmp.display());
    let output = Command::new(env!("CARGO_BIN_EXE_poste"))
        .args([
            "exec-file",
            sql_path.to_str().unwrap(),
            "--json",
            "--mode",
            "greedy",
            "--connection",
            &url,
            "--max-rows",
            "3",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("spawn poste exec-file");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tmpdir = tmp.parent().unwrap().to_str().unwrap().to_string();
    let actual: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| normalize(l, &tmpdir))
        .collect();
    std::fs::remove_file(&sql_path).ok();
    std::fs::remove_file(&tmp).ok();

    compare_or_update("max_rows_truncation", &actual);
    // spot-check the truncation markers the golden pins
    let select = actual
        .iter()
        .find(|l| l.contains("\"row_count\":8"))
        .expect("the SELECT event is present");
    let ev: Value = serde_json::from_str(select).unwrap();
    assert_eq!(ev["rows_truncated"], Value::Bool(true), "{select}");
    assert_eq!(ev["rows"].as_array().map(|r| r.len()), Some(3), "{select}");
    assert_eq!(
        ev["rows"][0],
        Value::Array(vec![Value::Number(1.into())]),
        "{select}"
    );
}
