use poste_core::sql_context::{detect_context_with_dialect, ContextType, SqlDialect};
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    dialect: String,
    sql: String,
    expect: Expect,
}

#[derive(Deserialize)]
struct Expect {
    ctx_type: String,
    ctx_data: Option<String>,
    ctx_schema: Option<String>,
    prefix: String,
    #[serde(default)]
    tables: Vec<TableExpect>,
    #[serde(default)]
    functions: Option<Vec<String>>,
    in_string: bool,
    in_comment: bool,
}

#[derive(Deserialize, Debug)]
struct TableExpect {
    name: String,
    alias: Option<String>,
    schema: Option<String>,
}

fn parse_dialect(s: &str) -> SqlDialect {
    match s {
        "postgres" => SqlDialect::Postgres,
        "mysql" => SqlDialect::MySql,
        "sqlite" => SqlDialect::Sqlite,
        _ => SqlDialect::Generic,
    }
}

fn extract_schema(ctx_type: &ContextType) -> Option<String> {
    match ctx_type {
        ContextType::DotColumn { schema, .. } => schema.clone(),
        ContextType::SchemaTable { schema } => Some(schema.clone()),
        _ => None,
    }
}

fn load_fixtures(path: &str) -> Vec<Fixture> {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read fixture file {}: {}", path, e));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse fixture file {}: {}", path, e))
}

fn run_fixture_file(path: &str) {
    let fixtures = load_fixtures(path);
    let file_stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path);

    for (i, f) in fixtures.iter().enumerate() {
        let cursor_pos = match f.sql.find('█') {
            Some(p) => p,
            None => panic!("[{}#{}] {}: Missing █ cursor marker", file_stem, i, f.name),
        };
        let sql = f.sql.replace('█', "");
        let dialect = parse_dialect(&f.dialect);

        let result = detect_context_with_dialect(&sql, cursor_pos, dialect);
        let result = result.unwrap_or_else(|| {
            panic!(
                "[{}#{}] {}: detect_context returned None",
                file_stem, i, f.name
            )
        });

        let prefix = &f.expect.prefix;
        let ctx_type_name = result.context_type.name();
        let ctx_data = result.context_type.data();
        let ctx_schema = extract_schema(&result.context_type);

        // Compare ctx_type
        assert_eq!(
            ctx_type_name, f.expect.ctx_type,
            "[{}#{}] {}: ctx_type mismatch\n  sql: {:?}\n  offset: {}",
            file_stem, i, f.name, sql, cursor_pos
        );

        // Compare ctx_data
        assert_eq!(
            ctx_data, f.expect.ctx_data,
            "[{}#{}] {}: ctx_data mismatch (context_type={:?})",
            file_stem, i, f.name, result.context_type
        );

        // Compare ctx_schema
        assert_eq!(
            ctx_schema, f.expect.ctx_schema,
            "[{}#{}] {}: ctx_schema mismatch",
            file_stem, i, f.name
        );

        // Compare prefix
        assert_eq!(
            &result.prefix, prefix,
            "[{}#{}] {}: prefix mismatch, expected {:?} got {:?}",
            file_stem, i, f.name, prefix, result.prefix
        );

        // Compare in_string
        assert_eq!(
            result.in_string, f.expect.in_string,
            "[{}#{}] {}: in_string mismatch",
            file_stem, i, f.name
        );

        // Compare in_comment
        assert_eq!(
            result.in_comment, f.expect.in_comment,
            "[{}#{}] {}: in_comment mismatch",
            file_stem, i, f.name
        );

        // Compare tables (order-insensitive)
        let result_tables: HashSet<(String, Option<String>, Option<String>)> = result
            .tables
            .iter()
            .map(|t| (t.name.clone(), t.alias.clone(), t.schema.clone()))
            .collect();
        let expect_tables: HashSet<(String, Option<String>, Option<String>)> = f
            .expect
            .tables
            .iter()
            .map(|t| (t.name.clone(), t.alias.clone(), t.schema.clone()))
            .collect();

        assert_eq!(
            result_tables,
            expect_tables,
            "[{}#{}] {}: tables mismatch\n  got:      {:?}\n  expected: {:?}",
            file_stem,
            i,
            f.name,
            result
                .tables
                .iter()
                .map(|t| (&t.name, &t.alias, &t.schema))
                .collect::<Vec<_>>(),
            f.expect
                .tables
                .iter()
                .map(|t| (&t.name, &t.alias, &t.schema))
                .collect::<Vec<_>>()
        );

        // Compare functions (skip if not in fixture)
        if let Some(ref expected_funcs) = f.expect.functions {
            let mut result_funcs: Vec<&str> = result.functions.iter().copied().collect();
            result_funcs.sort_unstable();
            let mut expected_sorted = expected_funcs.clone();
            expected_sorted.sort_unstable();
            assert_eq!(
                result_funcs, expected_sorted,
                "[{}#{}] {}: functions mismatch",
                file_stem, i, f.name
            );
        }
    }
}

#[test]
fn golden_basic_select() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/basic_select.json", dir));
}

#[test]
fn golden_directives() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/directives.json", dir));
}

#[test]
fn golden_strings_comments() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/strings_comments.json", dir));
}

#[test]
fn golden_dot_context() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/dot_context.json", dir));
}

#[test]
fn golden_dml_insert_update_delete() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/dml_insert_update_delete.json", dir));
}

#[test]
fn golden_statement_boundaries() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/statement_boundaries.json", dir));
}

#[test]
fn golden_cte_subquery_scope() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/cte_subquery_scope.json", dir));
}

#[test]
fn golden_ddl() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/ddl.json", dir));
}

#[test]
fn golden_where_complex() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/where_complex.json", dir));
}

#[test]
fn golden_special_statements() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/special_statements.json", dir));
}

#[test]
fn golden_dialect_postgres() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/dialect_postgres.json", dir));
}

#[test]
fn golden_dialect_mysql() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/dialect_mysql.json", dir));
}

#[test]
fn golden_dialect_sqlite() {
    let dir = fixture_dir();
    run_fixture_file(&format!("{}/dialect_sqlite.json", dir));
}

fn fixture_dir() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/tests/fixtures/sql_context", manifest_dir)
}
