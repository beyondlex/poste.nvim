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
