//! SQL-specific parsing: extract connection/database directives,
//! split multi-statement bodies, and strip comment directives.

use crate::Request;
use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

/// Result of parsing a SQL request body.
#[derive(Debug, Clone)]
pub struct SqlParseResult {
    /// The connection string (from Request, already resolved).
    pub connection: String,
    /// Optional database name from `-- @database` directive.
    pub database: Option<String>,
    /// Individual SQL statements, trimmed and variable-substituted.
    pub statements: Vec<String>,
}

/// Parse a SQL request body into structured components.
///
/// The body has already been through variable substitution in `parser.rs`,
/// so `{{var}}` references are already resolved.
pub fn parse_sql_request(request: &Request) -> Result<SqlParseResult> {
    let database = extract_database(request.body_str());
    let statements = split_statements(request.body_str());

    Ok(SqlParseResult {
        connection: request.connection.clone(),
        database,
        statements,
    })
}

/// Extract `-- @database <name>` directive from the body.
fn extract_database(body: &str) -> Option<String> {
    static DB_RE: OnceLock<Regex> = OnceLock::new();
    let re = DB_RE.get_or_init(|| {
        Regex::new(r"--\s*@database\s+(\S+)").expect("valid literal regex: @database")
    });
    for line in body.lines() {
        if let Some(caps) = re.captures(line) {
            return Some(caps[1].trim().to_string());
        }
    }
    None
}

/// Strip directive comment lines (`-- @connection`, `-- @database`, `-- @var = val`)
/// from the body, returning only the SQL content.
fn strip_directives(body: &str) -> String {
    static DIRECTIVE_RE: OnceLock<Regex> = OnceLock::new();
    let directive_re = DIRECTIVE_RE.get_or_init(|| {
        Regex::new(r"^\s*--\s*@\w+").expect("valid literal regex: directive comment")
    });
    body.lines()
        .filter(|line| !directive_re.is_match(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split SQL body into individual statements by semicolons.
///
/// Handles:
/// - Semicolons inside single-quoted strings (`'it''s; a test'`)
/// - Semicolons inside double-quoted identifiers (`"col;name"`)
/// - Semicolons inside Postgres dollar-quoted bodies (`$$...;...$$`,
///   `$fn$...;...$fn$`) — function/DO-block bodies routinely contain `;`
/// - Semicolons inside `--` line comments
/// - Semicolons inside `/* */` block comments
/// - Escaped quotes (`''` inside strings, `""` inside identifiers)
/// - Empty statements are filtered out
pub fn split_statements(body: &str) -> Vec<String> {
    let cleaned = strip_directives(body);
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = cleaned.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // Single-quoted string literal
            '\'' => {
                current.push(c);
                // Consume until closing quote (handle '' escapes)
                loop {
                    match chars.next() {
                        Some('\'') => {
                            current.push('\'');
                            // Check for escaped quote ''
                            if chars.peek() == Some(&'\'') {
                                current.push(chars.next().expect("peek confirmed quote exists"));
                            } else {
                                break;
                            }
                        }
                        Some(ch) => current.push(ch),
                        None => break, // Unterminated string
                    }
                }
            }
            // Double-quoted identifier
            '"' => {
                current.push(c);
                loop {
                    match chars.next() {
                        Some('"') => {
                            current.push('"');
                            // Check for escaped quote ""
                            if chars.peek() == Some(&'"') {
                                current.push(
                                    chars.next().expect("peek confirmed double-quote exists"),
                                );
                            } else {
                                break;
                            }
                        }
                        Some(ch) => current.push(ch),
                        None => break,
                    }
                }
            }
            // Line comment: -- ...
            '-' if chars.peek() == Some(&'-') => {
                chars.next(); // consume second -
                              // Consume until end of line (skip, not part of any statement)
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
            }
            // Block comment: /* ... */
            '/' if chars.peek() == Some(&'*') => {
                current.push(c);
                chars.next(); // consume *
                current.push('*');
                loop {
                    match chars.next() {
                        Some('*') if chars.peek() == Some(&'/') => {
                            current.push('*');
                            current.push('/');
                            chars.next(); // consume /
                            break;
                        }
                        Some(ch) => current.push(ch),
                        None => break,
                    }
                }
            }
            // Postgres dollar quote: `$$` or `$tag$`. Consume through the
            // closing tag so a `;` inside a function/DO-block body does not
            // split the statement. `$1`-style placeholders (and any `$`
            // not opening a valid tag) pass through untouched.
            '$' => {
                let mut tag = String::from("$");
                let mut is_tag = true;
                loop {
                    match chars.peek() {
                        Some('$') => {
                            tag.push('$');
                            chars.next();
                            break;
                        }
                        Some(&c) if tag.len() == 1 && (c.is_ascii_alphabetic() || c == '_') => {
                            tag.push(c);
                            chars.next();
                        }
                        Some(&c) if tag.len() > 1 && (c.is_ascii_alphanumeric() || c == '_') => {
                            tag.push(c);
                            chars.next();
                        }
                        _ => {
                            is_tag = false;
                            break;
                        }
                    }
                }
                if is_tag {
                    current.push_str(&tag);
                    let tag_chars: Vec<char> = tag.chars().collect();
                    let mut window: Vec<char> = Vec::new();
                    for ch in chars.by_ref() {
                        current.push(ch);
                        window.push(ch);
                        if window.len() > tag_chars.len() {
                            window.remove(0);
                        }
                        if window.len() == tag_chars.len() && window == tag_chars {
                            break; // closing tag consumed — back to normal scanning
                        }
                    }
                } else {
                    current.push_str(&tag);
                }
            }
            // Statement terminator
            ';' => {
                let stmt = current.trim().to_string();
                if !stmt.is_empty() {
                    statements.push(stmt);
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }

    // Last statement without trailing semicolon
    let stmt = current.trim().to_string();
    if !stmt.is_empty() {
        statements.push(stmt);
    }

    statements
}

/// A copy of `stmt` with the *contents* of string literals blanked to spaces,
/// for keyword-heuristic classification.
///
/// Classification code wants to answer "does this statement contain the
/// keyword RETURNING?" — but `upper.contains("RETURNING")` also matches the
/// word inside a literal (`UPDATE t SET note = 'returning'`), which used to
/// flip a DML statement onto the fetch path and lose its affected-row count.
/// Classify on this blanked view instead.
///
/// Same length in characters as the input (literals become spaces), so
/// offsets are stable. Handles single-quoted strings and double-quoted
/// identifiers with doubled-quote escapes (`''`, `""`), and Postgres
/// dollar-quoted strings (`$$...$$`, `$tag$...$tag$`). Backslash escapes
/// (`\'`) are intentionally NOT interpreted: this is a heuristic view, not a
/// parser, and the worst case (MySQL `\'` ends the blanked run early)
/// degrades to the pre-fix behavior for that one statement.
pub fn blank_string_literals(stmt: &str) -> String {
    let chars: Vec<char> = stmt.chars().collect();
    let mut out = vec![' '; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            out[i] = ' ';
            i += 1;
            while i < chars.len() {
                if chars[i] == c {
                    if i + 1 < chars.len() && chars[i + 1] == c {
                        i += 2; // doubled quote — still inside the literal
                    } else {
                        i += 1; // closing quote
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == '$' {
            // Postgres dollar quote: `$$` or `$tag$` with tag
            // [A-Za-z_][A-Za-z0-9_]*. Anything else is a literal `$`.
            let mut j = i + 1;
            if j < chars.len() && chars[j] != '$' {
                if !(chars[j].is_ascii_alphabetic() || chars[j] == '_') {
                    out[i] = '$';
                    i += 1;
                    continue;
                }
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
            }
            if j < chars.len() && chars[j] == '$' {
                let tag: Vec<char> = chars[i..=j].to_vec();
                let tag_len = tag.len();
                // Find the closing tag and blank everything through it.
                let mut k = i + tag_len;
                'closing: while k + tag_len <= chars.len() {
                    if chars[k..k + tag_len] == tag[..] {
                        // chars[i..k+tag_len] are already blanked via `out`
                        // init except this opening tag — blank it explicitly.
                        for slot in out[i..k + tag_len].iter_mut() {
                            *slot = ' ';
                        }
                        i = k + tag_len;
                        break 'closing;
                    }
                    k += 1;
                }
                if k + tag_len > chars.len() {
                    // Unterminated dollar quote: treat the `$` as literal and
                    // move on (matches split_statements' unterminated handling).
                    out[i] = '$';
                    i += 1;
                }
            } else {
                out[i] = '$';
                i += 1;
            }
        } else {
            out[i] = c;
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// Parse a TEXT/BLOB cell as JSON only when it is *structurally* JSON.
///
/// Returns `Some(parsed)` for values starting with `{` or `[`, else `None`.
/// Cell values that merely LOOK scalar-JSON (`123`, `null`, `true`) must stay
/// strings: SQLite has no column-level JSON type to gate on, so an unguarded
/// `serde_json::from_str` turns the text `"123"` into the number 123 and the
/// text `"null"` into SQL-style NULL — data misrepresentation, not parsing.
pub fn parse_json_cell(text: &str) -> Option<serde_json::Value> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        serde_json::from_str(trimmed).ok()
    } else {
        None
    }
}

/// Check if a SQL statement is a USE statement (e.g., `USE dbname`).
/// Returns the database name if so.
pub fn detect_use_statement(stmt: &str) -> Option<String> {
    let trimmed = stmt.trim();
    let upper = trimmed.to_uppercase();
    if upper.starts_with("USE ") {
        let rest = trimmed[4..].trim();
        // Strip trailing semicolon if present
        let db = rest.trim_end_matches(';').trim();
        // Strip quotes if present
        let db = db.trim_matches('`').trim_matches('"').trim_matches('\'');
        if !db.is_empty() {
            return Some(db.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Protocol;

    fn make_request(body: &str) -> Request {
        Request {
            name: Some("test".to_string()),
            protocol: Protocol::Postgres,
            connection: "postgres://localhost/test".to_string(),
            body: body.to_string().into_bytes(),
            raw_body: body.to_string(),
        }
    }

    #[test]
    fn test_extract_database() {
        assert_eq!(
            extract_database("-- @database mydb\nSELECT 1"),
            Some("mydb".to_string())
        );
        assert_eq!(extract_database("SELECT 1"), None);
    }

    #[test]
    fn test_split_simple() {
        let stmts = split_statements("SELECT 1; SELECT 2;");
        assert_eq!(stmts, vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn test_split_no_trailing_semicolon() {
        let stmts = split_statements("SELECT 1");
        assert_eq!(stmts, vec!["SELECT 1"]);
    }

    #[test]
    fn test_split_strips_directives() {
        let body =
            "-- @connection postgres://localhost/test\n-- @database mydb\nSELECT 1; SELECT 2;";
        let stmts = split_statements(body);
        assert_eq!(stmts, vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn test_split_semicolon_in_string() {
        let stmts = split_statements("SELECT 'hello;world'; SELECT 2;");
        assert_eq!(stmts, vec!["SELECT 'hello;world'", "SELECT 2"]);
    }

    #[test]
    fn test_split_escaped_quotes() {
        let stmts = split_statements("SELECT 'it''s; a test'; SELECT 2;");
        assert_eq!(stmts, vec!["SELECT 'it''s; a test'", "SELECT 2"]);
    }

    #[test]
    fn test_split_double_quoted_identifier() {
        let stmts = split_statements("SELECT \"col;name\" FROM t; SELECT 2;");
        assert_eq!(stmts, vec!["SELECT \"col;name\" FROM t", "SELECT 2"]);
    }

    #[test]
    fn test_split_line_comment() {
        let stmts = split_statements("SELECT 1; -- comment with ; inside\nSELECT 2;");
        assert_eq!(stmts, vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn test_split_block_comment() {
        let stmts = split_statements("SELECT /* ; */ 1; SELECT 2;");
        assert_eq!(stmts, vec!["SELECT /* ; */ 1", "SELECT 2"]);
    }

    #[test]
    fn test_split_empty_statements_filtered() {
        let stmts = split_statements("SELECT 1;;; SELECT 2;");
        assert_eq!(stmts, vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn test_split_dollar_quoted_body_with_semicolon() {
        // A `;` inside a dollar-quoted function/DO-block body must not split:
        // the pieces would each fail with a syntax error at execution time.
        let stmts =
            split_statements("CREATE FUNCTION f() AS $$ BEGIN SELECT 1; END $$ LANGUAGE plpgsql;");
        assert_eq!(
            stmts,
            vec!["CREATE FUNCTION f() AS $$ BEGIN SELECT 1; END $$ LANGUAGE plpgsql"]
        );
        assert_eq!(
            split_statements("INSERT INTO t VALUES ($$a;b$$);"),
            vec!["INSERT INTO t VALUES ($$a;b$$)"]
        );
        // Tagged dollar quotes.
        assert_eq!(
            split_statements("DO $fn$ LOOP x; END LOOP; $fn$; SELECT 2;"),
            vec!["DO $fn$ LOOP x; END LOOP; $fn$", "SELECT 2"]
        );
    }

    #[test]
    fn test_split_dollar_non_tags_untouched() {
        // `$1` positional parameters and stray `$` stay literal, like
        // blank_string_literals.
        assert_eq!(
            split_statements("SELECT $1, $2 FROM t WHERE x = 'a;b';"),
            vec!["SELECT $1, $2 FROM t WHERE x = 'a;b'"]
        );
        assert_eq!(split_statements("SELECT a $ b;"), vec!["SELECT a $ b"]);
        // Truncated tag at end of body.
        assert_eq!(split_statements("SELECT $fn"), vec!["SELECT $fn"]);
    }

    #[test]
    fn test_detect_use_statement() {
        assert_eq!(detect_use_statement("USE mydb"), Some("mydb".to_string()));
        assert_eq!(detect_use_statement("use mydb;"), Some("mydb".to_string()));
        assert_eq!(detect_use_statement("USE `mydb`"), Some("mydb".to_string()));
        assert_eq!(
            detect_use_statement("USE \"mydb\""),
            Some("mydb".to_string())
        );
        assert_eq!(detect_use_statement("SELECT 1"), None);
        assert_eq!(detect_use_statement("USELESS"), None);
    }

    // ---- blank_string_literals (keyword-heuristic view) ----

    fn has_returning(stmt: &str) -> bool {
        blank_string_literals(stmt)
            .to_uppercase()
            .contains("RETURNING")
    }

    #[test]
    fn test_blank_literals_returning_in_string_not_matched() {
        // The bug this helper exists for: a literal containing "returning"
        // must not flip DML onto the fetch path.
        assert!(!has_returning(
            "INSERT INTO log (msg) VALUES ('returning item')"
        ));
        assert!(!has_returning(
            "UPDATE t SET note = 'returning' WHERE id = 1"
        ));
        assert!(!has_returning(
            "UPDATE t SET \"returning\" = true WHERE id = 1"
        ));
    }

    #[test]
    fn test_blank_literals_real_returning_clause_still_matched() {
        assert!(has_returning("INSERT INTO t (a) VALUES (1) RETURNING id"));
        assert!(has_returning("update t set a = 1 returning *"));
        assert!(has_returning("DELETE FROM t WHERE id = 1 RETURNING id"));
    }

    #[test]
    fn test_blank_literals_preserves_non_literal_text() {
        assert_eq!(blank_string_literals("SELECT 1"), "SELECT 1");
        // Entire literal (delimiters included) becomes spaces, same char length.
        assert_eq!(blank_string_literals("SELECT 'abc', 2"), "SELECT      , 2");
        assert_eq!(
            blank_string_literals("SELECT \"a b\", 2").chars().count(),
            "SELECT \"a b\", 2".chars().count()
        );
    }

    #[test]
    fn test_blank_literals_doubled_quote_escape() {
        // 'it''s returning' — the doubled quote stays inside the literal.
        assert_eq!(blank_string_literals("'it''s'"), "       ");
        assert!(!has_returning("UPDATE t SET a = 'it''s returning'"));
        assert_eq!(blank_string_literals("\"a\"\"b\""), "      ");
    }

    #[test]
    fn test_blank_literals_unterminated_string() {
        // Unterminated literal: blank to end, no panic.
        assert_eq!(blank_string_literals("SELECT 'abc"), "SELECT     ");
        assert_eq!(blank_string_literals("'"), " ");
    }

    #[test]
    fn test_blank_literals_dollar_quoted() {
        assert!(!has_returning(
            "CREATE FUNCTION f() AS $$ SELECT 'returning'; $$ LANGUAGE sql"
        ));
        // Bare RETURNING inside a dollar-quoted body is blanked too.
        assert!(!has_returning(
            "CREATE FUNCTION f() AS $$ BEGIN RETURNING x; END $$ LANGUAGE plpgsql"
        ));
        assert!(has_returning("INSERT INTO t VALUES (1) RETURNING id"));
        // Tagged dollar quotes.
        assert!(!has_returning("CREATE FUNCTION f() AS $fn$ RETURNING $fn$"));
        // Non-quotes: $1 positional parameter stays as-is.
        assert_eq!(blank_string_literals("WHERE x = $1"), "WHERE x = $1");
        // Unterminated dollar quote: `$` treated as literal.
        assert_eq!(blank_string_literals("a $$ b"), "a $$ b");
    }

    #[test]
    fn test_blank_literals_multiline_string() {
        let stmt = "INSERT INTO t VALUES ('multi\nline returning') RETURNING id";
        let blanked = blank_string_literals(stmt);
        assert!(!blanked.contains("returning"));
        assert!(blanked.to_uppercase().contains("RETURNING"));
        // Same char length (newlines inside the literal become spaces).
        assert_eq!(blanked.chars().count(), stmt.chars().count());
    }

    // ---- parse_json_cell (structural-JSON-only cell parsing) ----

    #[test]
    fn test_parse_json_cell_scalar_text_stays_string() {
        // Scalar-looking text must NOT be coerced — the text "null" parsed
        // with unguarded from_str becomes SQL-NULL to the reader.
        assert!(parse_json_cell("123").is_none());
        assert!(parse_json_cell("null").is_none());
        assert!(parse_json_cell("true").is_none());
        assert!(parse_json_cell("-1.5").is_none());
    }

    #[test]
    fn test_parse_json_cell_structural_parses() {
        assert_eq!(
            parse_json_cell(r#"{"a": 1}"#),
            Some(serde_json::json!({"a": 1}))
        );
        assert_eq!(parse_json_cell("[1, 2]"), Some(serde_json::json!([1, 2])));
        // Leading whitespace is tolerated (values may carry padding).
        assert_eq!(parse_json_cell("  [1]"), Some(serde_json::json!([1])));
    }

    #[test]
    fn test_parse_json_cell_broken_structural_is_none() {
        // Malformed structural JSON falls back to the raw string.
        assert!(parse_json_cell("{not json").is_none());
        assert!(parse_json_cell("[1,").is_none());
    }

    #[test]
    fn test_parse_sql_request_full() {
        let req = make_request(
            "-- @connection postgres://localhost/test\n\
             -- @database mydb\n\
             SELECT * FROM users;\n\
             SELECT * FROM orders;",
        );
        let result = parse_sql_request(&req).unwrap();
        assert_eq!(result.connection, "postgres://localhost/test");
        assert_eq!(result.database, Some("mydb".to_string()));
        assert_eq!(result.statements.len(), 2);
        assert_eq!(result.statements[0], "SELECT * FROM users");
        assert_eq!(result.statements[1], "SELECT * FROM orders");
    }

    #[test]
    fn test_parse_sql_request_no_database() {
        let req = make_request("SELECT 1");
        let result = parse_sql_request(&req).unwrap();
        assert_eq!(result.database, None);
        assert_eq!(result.statements, vec!["SELECT 1"]);
    }
}
