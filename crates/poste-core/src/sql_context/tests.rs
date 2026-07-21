use super::*;

// ---- Tokenizer ----

#[test]
fn test_tokenize_basic() {
    let tokens = tokenize("SELECT * FROM users WHERE id = 1");
    assert!(!tokens.is_empty());
    let src = "SELECT * FROM users WHERE id = 1";
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Keyword && t.text(src) == "SELECT"));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Keyword && t.text(src) == "FROM"));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Keyword && t.text(src) == "WHERE"));
    assert!(tokens
        .iter()
        .any(|t| matches!(t.kind, TokenKind::Ident) && t.text(src) == "users"));
}

#[test]
fn test_tokenize_string_with_semicolon() {
    let tokens = tokenize("SELECT 'hello;world'");
    assert!(!tokens.iter().any(|t| t.kind == TokenKind::Semi));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::StrLit));
}

#[test]
fn test_tokenize_escaped_quotes() {
    let tokens = tokenize("SELECT 'it''s; a test'");
    assert!(!tokens.iter().any(|t| t.kind == TokenKind::Semi));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::StrLit));
}

#[test]
fn test_tokenize_line_comment() {
    let tokens = tokenize("SELECT 1; -- comment with ;\nSELECT 2");
    let semis: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Semi)
        .collect();
    assert_eq!(semis.len(), 1);
    assert!(tokens.iter().any(|t| t.kind == TokenKind::LineComment));
}

#[test]
fn test_tokenize_block_comment() {
    let tokens = tokenize("SELECT /* ; */ 1");
    assert!(!tokens.iter().any(|t| t.kind == TokenKind::Semi));
    assert!(tokens.iter().any(|t| t.kind == TokenKind::BlockComment));
}

#[test]
fn test_tokenize_hyphenated_identifier() {
    let tokens = tokenize("SELECT * FROM posts-web_vitals");
    let src = "SELECT * FROM posts-web_vitals";
    assert!(tokens
        .iter()
        .any(|t| matches!(t.kind, TokenKind::Ident) && t.text(src) == "posts-web_vitals"));
}

#[test]
fn test_tokenize_subtraction_operator() {
    let tokens = tokenize("x - 1");
    let src = "x - 1";
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Ident && t.text(src) == "x"));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Op && t.text(src) == "-"));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::NumLit && t.text(src) == "1"));
}

#[test]
fn test_tokenize_inline_block_comment() {
    let tokens = tokenize("SELECT /* inline */ col FROM t");
    assert!(tokens.iter().any(|t| t.kind == TokenKind::BlockComment));
    let src = "SELECT /* inline */ col FROM t";
    assert!(tokens
        .iter()
        .any(|t| matches!(t.kind, TokenKind::Ident) && t.text(src) == "col"));
}

#[test]
fn test_tokenize_multiple_block_comments() {
    let tokens = tokenize("SELECT /* a */ 1 /* b */ WHERE");
    let src = "SELECT /* a */ 1 /* b */ WHERE";
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::BlockComment)
            .count(),
        2
    );
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Keyword && t.text(src) == "WHERE"));
}

#[test]
fn test_tokenize_adjacent_block_comments() {
    let tokens = tokenize("SELECT /**/1/**/FROM t");
    let src = "SELECT /**/1/**/FROM t";
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::BlockComment)
            .count(),
        2
    );
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Keyword && t.text(src) == "FROM"));
}

// ---- Context detection: basic DML ----

#[test]
fn test_detect_keyword_by_default() {
    let result = detect_context("", 0).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_table_after_from_ident_trailing_ws() {
    let result = detect_context("SELECT * FROM a", 15).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
    assert_eq!(result.tables.len(), 1);
    assert_eq!(result.prefix, "a");
}

#[test]
fn test_detect_keyword_after_table_name_with_trailing_ws() {
    // SELECT * FROM posts<space> — cursor is past the table name on whitespace,
    // should suggest keywords (WHERE, GROUP BY, ORDER BY, etc.), not tables.
    let result = detect_context("SELECT * FROM posts ", 20).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "After table name with trailing space, should suggest keywords, got {:?}",
        result.context_type
    );
    assert_eq!(
        result.prefix, "",
        "no prefix when cursor is on whitespace after table"
    );
}

#[test]
fn test_detect_table_after_from_comments_and_trailing_ws() {
    let sql = "-- @connection my-blog\n-- @database blog\n\nSELECT * FROM a";
    let result = detect_context(sql, 59).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
    assert_eq!(result.prefix, "a");
}

#[test]
fn test_detect_s_prefix_returns_keyword() {
    let result = detect_context("s", 1).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
    assert_eq!(result.prefix, "s");
}

#[test]
fn test_detect_select_star_f_prefix_returns_keyword() {
    let result = detect_context("select * f", 10).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
    assert_eq!(result.prefix, "f");
}

#[test]
fn test_detect_table_after_from() {
    let result = detect_context("SELECT * FROM ", 14).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_table_after_join() {
    let result = detect_context("SELECT * FROM users JOIN ", 25).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_column_after_where() {
    let result = detect_context("SELECT * FROM users WHERE ", 25).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_column_where_semicolon_space() {
    let result = detect_context("SELECT * FROM users WHERE ;", 25).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_column_where_semicolon_on_semi() {
    let result = detect_context("SELECT * FROM users WHERE ;", 26).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_column_where_semicolon_after() {
    let result = detect_context("SELECT * FROM users WHERE ;", 27).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_on_column() {
    let result = detect_context("SELECT * FROM users u JOIN posts p ON ", 39).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_and_column() {
    let result = detect_context("SELECT * FROM users WHERE id = 1 AND ", 37).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_where_id_completed_keyword() {
    let result = detect_context("SELECT * FROM users WHERE id ", 29).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_having_col_completed_keyword() {
    let result =
        detect_context("SELECT id, COUNT(*) FROM users GROUP BY id HAVING cnt ", 56).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_comma_after_column() {
    let result = detect_context("SELECT id, name, ", 17).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_inside_string_returns_string_type() {
    let result = detect_context("SELECT 'hello world'", 10).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::String,
        "cursor inside string should return String type"
    );
    assert!(result.in_string, "in_string should be true");
    assert!(!result.in_comment, "in_comment should be false");
}

#[test]
fn test_detect_comment_where_returns_comment_type() {
    let result = detect_context("-- WHERE ", 9).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Comment,
        "cursor in line comment should return Comment type"
    );
    assert!(result.in_comment, "in_comment should be true");
    assert!(!result.in_string, "in_string should be false");
}

#[test]
fn test_detect_comment_from_returns_comment_type() {
    let result = detect_context("-- FROM ", 8).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Comment,
        "cursor in line comment should return Comment type"
    );
    assert!(result.in_comment, "in_comment should be true");
    assert!(!result.in_string, "in_string should be false");
}

#[test]
fn test_detect_connection_directive() {
    // @connection is now handled by Lua; Rust returns Keyword as safety net fallback
    let result = detect_context("@connection ", 12).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_use_database() {
    let result = detect_context("USE ", 4).unwrap();
    assert_eq!(result.context_type, ContextType::Database);
}

#[test]
fn test_edge_use_prefix() {
    let result = detect_context("USE mydb", 8).unwrap();
    assert_eq!(result.context_type, ContextType::Database);
}

// ---- Dot-column ----

#[test]
fn test_detect_dot_column() {
    let result = detect_context("SELECT users.", 13).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::DotColumn {
            table: "users".into(),
            schema: None
        }
    );
}

#[test]
fn test_detect_dot_column_alias() {
    let result = detect_context("SELECT * FROM users u WHERE u.", 29).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::DotColumn {
            table: "u".into(),
            schema: None
        }
    );
}

#[test]
fn test_detect_dot_column_alias_in_select() {
    let sql = "SELECT p.*, a. from posts p LEFT JOIN authors a on a.id = p.author_id;";
    let result = detect_context(sql, 14).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::DotColumn {
            table: "a".into(),
            schema: None
        }
    );
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "authors" && t.alias == Some("a".into())));
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "posts" && t.alias == Some("p".into())));
}

#[test]
fn test_detect_dot_column_schema_qualified() {
    let result = detect_context("SELECT auth.users.", 18).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::DotColumn {
            table: "users".into(),
            schema: Some("auth".into())
        }
    );
}

#[test]
fn test_detect_dot_column_schema_qualified_alias() {
    let result = detect_context("SELECT * FROM public.users u WHERE u.", 38).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::DotColumn {
            table: "u".into(),
            schema: None
        }
    );
    assert!(result.tables.iter().any(|t| {
        t.name == "users" && t.alias == Some("u".into()) && t.schema == Some("public".into())
    }));
}

#[test]
fn test_detect_dot_column_schema_qualified_bare_table() {
    let result = detect_context("SELECT * FROM auth.users WHERE users.", 39).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::DotColumn {
            table: "users".into(),
            schema: None
        }
    );
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "users" && t.schema == Some("auth".into())));
}

#[test]
fn test_detect_schema_table_after_from_dot() {
    let result = detect_context("SELECT * FROM inventory.", 24).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::SchemaTable {
            schema: "inventory".into()
        }
    );
}

#[test]
fn test_detect_schema_table_with_prefix() {
    let result = detect_context("SELECT * FROM inventory.us", 27).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::SchemaTable {
            schema: "inventory".into()
        }
    );
    assert_eq!(result.prefix, "us");
}

#[test]
fn test_detect_schema_table_after_join_dot() {
    let result = detect_context("SELECT * FROM t JOIN inventory.", 32).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::SchemaTable {
            schema: "inventory".into()
        }
    );
}

#[test]
fn test_detect_column_with_schema_qualified_table() {
    let result = detect_context("SELECT * FROM auth.users WHERE ", 31).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
    assert!(result
        .tables
        .iter()
        .any(|t| { t.name == "users" && t.schema == Some("auth".into()) }));
}

#[test]
fn test_detect_multi_schema_tables() {
    let result = detect_context(
        "SELECT * FROM public.users JOIN auth.users ON public.users.id = auth.users.user_id WHERE ",
        88,
    )
    .unwrap();
    assert_eq!(result.context_type, ContextType::Column);
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "users" && t.schema == Some("public".into())));
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "users" && t.schema == Some("auth".into())));
}

// ---- INSERT column ----

#[test]
fn test_detect_insert_column() {
    let result = detect_context("INSERT INTO users (", 19).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::InsertColumn {
            table: "users".into()
        }
    );
}

#[test]
fn test_detect_insert_column_open_paren() {
    let result = detect_context("INSERT INTO posts ()", 18).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::InsertColumn {
            table: "posts".into()
        }
    );
}

#[test]
fn test_detect_insert_column_closed_paren() {
    let result = detect_context("INSERT INTO posts ()", 19).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::InsertColumn {
            table: "posts".into()
        }
    );
}

#[test]
fn test_detect_insert_column_after_paren() {
    let result = detect_context("INSERT INTO posts ()", 20).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::InsertColumn {
            table: "posts".into()
        }
    );
}

// ---- Various clause contexts ----

#[test]
fn test_detect_updates_set_column() {
    let result = detect_context("UPDATE users SET ", 17).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_edge_delete_from() {
    let result = detect_context("DELETE FROM ", 12).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_edge_having() {
    let result = detect_context("SELECT id, COUNT(*) FROM users GROUP BY id HAVING ", 47).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_edge_order_by() {
    let result = detect_context("SELECT * FROM users ORDER BY ", 30).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_where_in_values() {
    let result = detect_context("SELECT * FROM users WHERE id IN (1, 2, ", 41).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_where_between() {
    let result = detect_context("SELECT * FROM users WHERE id BETWEEN ", 40).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_where_not_between() {
    let result = detect_context("SELECT * FROM users WHERE id NOT BETWEEN ", 44).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_where_between_and() {
    let result = detect_context("SELECT * FROM users WHERE id BETWEEN 1 AND ", 46).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "AND after BETWEEN should suggest columns"
    );
}

#[test]
fn test_detect_where_like() {
    let result = detect_context("SELECT * FROM users WHERE name LIKE ", 37).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_where_not_like() {
    let result = detect_context("SELECT * FROM users WHERE name NOT LIKE ", 41).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_where_is_null() {
    let result = detect_context("SELECT * FROM users WHERE status IS ", 37).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_where_not_keyword() {
    let result = detect_context("SELECT * FROM users WHERE id NOT ", 34).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "NOT after column should suggest IN/LIKE/BETWEEN/IS"
    );
}

#[test]
fn test_detect_where_is_not_null_column() {
    let result = detect_context("SELECT * FROM users WHERE status IS NOT ", 41).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "IS NOT should suggest NULL/columns"
    );
}

#[test]
fn test_detect_where_not_exists_keyword() {
    let result = detect_context("SELECT * FROM users WHERE NOT ", 30).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "WHERE NOT should suggest columns"
    );
}

#[test]
fn test_detect_where_eq_column() {
    let result = detect_context("SELECT * FROM users WHERE id = ", 34).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "WHERE col = should suggest keyword (AND/OR/ORDER BY)"
    );
}

#[test]
fn test_detect_where_gt_column() {
    let result = detect_context("SELECT * FROM users WHERE age > ", 35).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "WHERE col > should suggest keyword"
    );
}

// ---- DDL ----

#[test]
fn test_detect_create_index() {
    let result = detect_context("CREATE INDEX ", 13).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_drop_index() {
    let result = detect_context("DROP INDEX ", 11).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_create_view() {
    let result = detect_context("CREATE VIEW ", 12).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_edge_create_table() {
    let result = detect_context("CREATE TABLE ", 13).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_alter_table() {
    let result = detect_context("ALTER TABLE ", 12).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_alter_table_add_column_datatype() {
    let result = detect_context("ALTER TABLE users ADD COLUMN age ", 33).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::DataType,
        "After ADD COLUMN col_name should suggest data types"
    );
}

#[test]
fn test_detect_drop_table() {
    let result = detect_context("DROP TABLE ", 11).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_truncate_table() {
    let result = detect_context("TRUNCATE TABLE ", 15).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

// ---- Window functions ----

#[test]
fn test_detect_over_keyword() {
    let result = detect_context("SELECT RANK() OVER ", 19).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_window_partition_by() {
    let result = detect_context("SELECT RANK() OVER (PARTITION BY ", 33).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_window_partition_by_with_prefix() {
    let result = detect_context("SELECT RANK() OVER (PARTITION BY dep", 34).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_window_order_by() {
    let result = detect_context("SELECT RANK() OVER (ORDER BY ", 29).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

// ---- Set operations ----

#[test]
fn test_detect_union() {
    let result = detect_context("SELECT id FROM users UNION ", 27).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_union_all() {
    let result = detect_context("SELECT id FROM users UNION ALL ", 31).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_intersect() {
    let result = detect_context("SELECT id FROM users INTERSECT ", 31).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_except() {
    let result = detect_context("SELECT id FROM users EXCEPT ", 28).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

// ---- CASE / functions ----

#[test]
fn test_detect_case_when() {
    let result = detect_context("SELECT CASE WHEN ", 16).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_case_then() {
    let result = detect_context("SELECT CASE WHEN id = 1 THEN ", 28).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_case_else() {
    let result = detect_context("SELECT CASE WHEN id = 1 THEN 'a' ELSE ", 38).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

// ---- Function call parens — should suggest Column ----

#[test]
fn test_detect_function_paren_coalesce() {
    let result = detect_context("SELECT COALESCE(", 16).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "COALESCE( is a function call → Column"
    );
}

#[test]
fn test_detect_function_paren_from_unixtime() {
    let result = detect_context("SELECT FROM_UNIXTIME(", 21).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "FROM_UNIXTIME( is a function call → Column"
    );
}

#[test]
fn test_detect_function_paren_concat() {
    let result = detect_context("SELECT CONCAT(", 14).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "CONCAT( is a function call → Column"
    );
}

#[test]
fn test_detect_function_paren_nested() {
    let result = detect_context("SELECT ROUND(AVG(", 17).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "Nested function call AVG( → Column"
    );
}

#[test]
fn test_detect_function_paren_after_comma() {
    let result = detect_context("SELECT COALESCE(col1, ", 22).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "Inside COALESCE( after comma → Column"
    );
}

#[test]
fn test_detect_function_paren_with_tables() {
    let result =
        detect_context("SELECT COALESCE(col1, col2) FROM users WHERE COALESCE(", 53).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "COALESCE( in WHERE should suggest columns"
    );
    assert!(
        result.tables.iter().any(|t| t.name == "users"),
        "Tables should include 'users' from outer query"
    );
}

#[test]
fn test_detect_where_paren_expression() {
    let result = detect_context("SELECT * FROM users WHERE (", 27).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "WHERE ( is expression grouping → Column"
    );
}

#[test]
fn test_detect_and_paren_expression() {
    let result = detect_context("SELECT * FROM users WHERE id = 1 AND (", 38).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "AND ( is expression grouping → Column"
    );
}

#[test]
fn test_detect_subquery_paren_after_from() {
    let result = detect_context("SELECT * FROM (", 15).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "FROM ( is subquery start → Keyword"
    );
}

#[test]
fn test_detect_subquery_paren_after_in() {
    let result = detect_context("SELECT * FROM users WHERE id IN (", 34).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "IN ( is subquery → Keyword"
    );
}

#[test]
fn test_detect_subquery_paren_after_exists() {
    let result = detect_context("SELECT * FROM users WHERE EXISTS (", 35).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "EXISTS ( is subquery → Keyword"
    );
}

#[test]
fn test_detect_coalesce() {
    let result = detect_context("SELECT COALESCE(", 16).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_nullif() {
    let result = detect_context("SELECT NULLIF(", 14).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_count() {
    let result = detect_context("SELECT COUNT(", 13).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_avg() {
    let result = detect_context("SELECT AVG(", 11).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_sum() {
    let result = detect_context("SELECT SUM(", 11).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_extract() {
    let result = detect_context("SELECT EXTRACT(", 15).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_concat() {
    let result = detect_context("SELECT CONCAT(", 14).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_substring() {
    let result = detect_context("SELECT SUBSTRING(", 17).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_cast_as() {
    let result = detect_context("SELECT CAST(", 12).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

// ---- RETURNING / ON CONFLICT ----

#[test]
fn test_detect_returning() {
    let result = detect_context("DELETE FROM users WHERE id = 1 RETURNING ", 42).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "RETURNING should suggest columns"
    );
}

#[test]
fn test_detect_on_conflict_do_update_set() {
    let result = detect_context(
        "INSERT INTO users (id) VALUES (1) ON CONFLICT (id) DO UPDATE SET ",
        62,
    )
    .unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "ON CONFLICT DO UPDATE SET should suggest columns"
    );
}

#[test]
fn test_detect_insert_on_conflict() {
    let result = detect_context("INSERT INTO users VALUES (1) ON CONFLICT DO NOTHING", 54).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

// ---- Transaction / Misc ----

#[test]
fn test_detect_begin_keyword() {
    let result = detect_context("BEGIN ", 6).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_dialect_functions_pg() {
    let result =
        detect_context_with_dialect("SELECT * FROM users WHERE ", 26, SqlDialect::Postgres)
            .unwrap();
    for f in &result.functions {
        assert_ne!(*f, "CRC32", "PG should not include CRC32");
    }
    assert!(
        result.functions.contains(&"STRING_AGG"),
        "PG should include STRING_AGG"
    );
    assert!(
        result.functions.contains(&"COUNT"),
        "PG should include COUNT"
    );
}

#[test]
fn test_detect_dialect_functions_mysql() {
    let result =
        detect_context_with_dialect("SELECT * FROM users WHERE ", 26, SqlDialect::MySql).unwrap();
    for f in &result.functions {
        assert_ne!(*f, "STRING_AGG", "MySQL should not include STRING_AGG");
    }
    assert!(
        result.functions.contains(&"CRC32"),
        "MySQL should include CRC32"
    );
    assert!(
        result.functions.contains(&"COUNT"),
        "MySQL should include COUNT"
    );
}

#[test]
fn test_detect_dialect_functions_unknown_defaults_all() {
    let result = detect_context("SELECT * FROM users WHERE ", 26).unwrap();
    assert!(
        result.functions.contains(&"CRC32"),
        "Default should include all functions"
    );
    assert!(
        result.functions.contains(&"STRING_AGG"),
        "Default should include all functions"
    );
}

#[test]
fn test_detect_commit_keyword() {
    let result = detect_context("COMMIT ", 7).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_rollback_keyword() {
    let result = detect_context("ROLLBACK ", 9).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_savepoint() {
    let result = detect_context("SAVEPOINT ", 10).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_explain() {
    let result = detect_context("EXPLAIN ", 8).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_set_statement() {
    let result = detect_context("SET statement_timeout = ", 24).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_update_set_comma() {
    let result = detect_context("UPDATE users SET name = 'x', ", 30).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_update_set_multi_column() {
    let result = detect_context("UPDATE users SET name = 'x', age = ", 35).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_update_set_three_columns() {
    let result = detect_context("UPDATE posts SET author_id=1, slug='', ", 39).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "After comma in SET clause, should suggest next column"
    );
}

#[test]
fn test_detect_update_set_where_keyword() {
    let result = detect_context("UPDATE posts SET slug='', author_id='', bio='' w", 48).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_grant() {
    let result = detect_context("GRANT SELECT ON ", 16).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_grant_on_prefix() {
    let result = detect_context("GRANT SELECT ON us", 19).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_revoke_on() {
    let result = detect_context("REVOKE ALL ON ", 14).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_revoke() {
    let result = detect_context("REVOKE ", 7).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_copy_from() {
    let result = detect_context("COPY ", 5).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_copy_column() {
    let result = detect_context("COPY users (", 12).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::InsertColumn {
            table: "users".to_string()
        }
    );
}

// ---- SHOW statement ----

#[test]
fn test_detect_show_tables() {
    let result = detect_context("SHOW TABLES ", 12).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_show_tables_prefix() {
    let result = detect_context("SHOW TABLES", 11).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_show_databases() {
    let result = detect_context("SHOW DATABASES ", 15).unwrap();
    assert_eq!(result.context_type, ContextType::Database);
}

#[test]
fn test_detect_show_schemas() {
    let result = detect_context("SHOW SCHEMAS ", 13).unwrap();
    assert_eq!(result.context_type, ContextType::Database);
}

#[test]
fn test_detect_show_columns_from() {
    let result = detect_context("SHOW COLUMNS FROM ", 18).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_show_fields_from() {
    let result = detect_context("SHOW FIELDS FROM ", 17).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_show_columns_from_table_prefix() {
    let result = detect_context("SHOW COLUMNS FROM us", 20).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_show_bare_keyword() {
    let result = detect_context("SHOW ", 5).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_show_tables_semicolon_boundary_new_stmt_s() {
    // show tables; is a completed statement. `s` on next line is a new statement.
    let result = detect_context("show tables;\ns", 14).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "After show tables;, new 's' should be Keyword, got {:?}",
        result.context_type
    );
    assert_eq!(result.prefix, "s");
}

#[test]
fn test_detect_show_tables_semicolon_boundary_new_stmt_select() {
    // show tables; on one line, SELECT s on the next.
    let result = detect_context("show tables;\nSELECT s", 21).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "After show tables;, SELECT s should be Column, got {:?}",
        result.context_type
    );
    assert_eq!(result.prefix, "s");
}

#[test]
fn test_detect_show_tables_no_semicolon_still_works() {
    // Without semicolon, SHOW TABLES should still suggest tables.
    let result = detect_context("SHOW TABLES ", 12).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_analyze() {
    let result = detect_context("ANALYZE ", 8).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_vacuum() {
    let result = detect_context("VACUUM ", 7).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_call_keyword() {
    let result = detect_context("CALL ", 5).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_do_keyword() {
    let result = detect_context("DO ", 3).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_prepare_keyword() {
    let result = detect_context("PREPARE ", 8).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_execute_keyword() {
    let result = detect_context("EXECUTE ", 8).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_explain_analyze() {
    let result = detect_context("EXPLAIN ANALYZE ", 16).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_listen_keyword() {
    let result = detect_context("LISTEN ", 7).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_notify_keyword() {
    let result = detect_context("NOTIFY ", 7).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_lock_table() {
    let result = detect_context("LOCK TABLE ", 11).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

// ---- FOR UPDATE / FOR SHARE ----

#[test]
fn test_detect_for_update_of() {
    let result = detect_context("SELECT * FROM users FOR UPDATE OF ", 34).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_for_update_of_prefix() {
    let result = detect_context("SELECT * FROM users FOR UPDATE OF ord", 38).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_for_share_of() {
    let result = detect_context("SELECT * FROM users FOR SHARE OF ", 33).unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_for_update_nowait() {
    let result = detect_context("SELECT * FROM users FOR UPDATE NOWAIT", 37).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_for_update_skip_locked() {
    let result = detect_context("SELECT * FROM users FOR UPDATE SKIP LOCKED", 42).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

// ---- LATERAL / INSERT INTO SELECT ----

#[test]
fn test_detect_lateral_join() {
    let result = detect_context(
        "SELECT * FROM users u JOIN LATERAL (SELECT * FROM orders WHERE ",
        60,
    )
    .unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_insert_into_select() {
    let result = detect_context("INSERT INTO users (id, name) SELECT ", 37).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

// ---- Complex WHERE ----

#[test]
fn test_detect_where_parenthesized_and() {
    let result =
        detect_context("SELECT * FROM users WHERE (id = 1 AND name = 'a') AND ", 52).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_where_parenthesized_or() {
    let result = detect_context("SELECT * FROM users WHERE (a = 1 OR b = 2) AND ", 45).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "AND after parenthesized condition should suggest columns"
    );
}

#[test]
fn test_detect_where_deeply_parenthesized() {
    let result =
        detect_context("SELECT * FROM users WHERE ((a = 1) AND (b = 2)) AND ", 49).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

// ---- subquery contexts ----

#[test]
fn test_detect_cursor_after_subquery_where() {
    let result = detect_context("SELECT * FROM (SELECT * FROM items) AS sub WHERE ", 49).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_cursor_inside_subquery_exists() {
    let result = detect_context(
        "SELECT * FROM users WHERE EXISTS (SELECT 1 FROM secret WHERE ",
        55,
    )
    .unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_inside_subquery_in_in_clause() {
    let result = detect_context(
        "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders WHERE ",
        57,
    )
    .unwrap();
    assert_eq!(result.context_type, ContextType::Table);
}

#[test]
fn test_detect_after_deeply_nested_subquery() {
    let result = detect_context(
        "SELECT * FROM (SELECT * FROM (SELECT * FROM deep) AS mid) AS outer WHERE ",
        74,
    )
    .unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

// ---- table from subquery ----

#[test]
fn test_detect_table_from_update_in_subquery() {
    let result = detect_context("SELECT * FROM (SELECT * FROM items) AS sub WHERE ", 49).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

// ---- CTE ----

#[test]
fn test_detect_after_cte_select() {
    let result = detect_context(
        "WITH cte AS (SELECT * FROM users) SELECT * FROM cte WHERE ",
        57,
    )
    .unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

// ---- SELECT variations ----

#[test]
fn test_detect_select_star_returns_keyword() {
    let result = detect_context("SELECT * ", 9).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_select_expr_returns_keyword() {
    let result = detect_context("SELECT col ", 11).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_select_star_comma_returns_column() {
    let result = detect_context("SELECT id, *, ", 14).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_select_comma_with_prefix() {
    let result = detect_context("SELECT *, col", 13).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_select_with_prefix_returns_keyword() {
    let result = detect_context("SELECT col ", 11).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_select_star_typed_f_returns_keyword() {
    let result = detect_context("select * f", 10).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
}

#[test]
fn test_detect_typed_s_returns_keyword() {
    let result = detect_context("s", 1).unwrap();
    assert_eq!(result.context_type, ContextType::Keyword);
    assert_eq!(result.prefix, "s");
}

#[test]
fn test_detect_select_distinct() {
    let result = detect_context("SELECT DISTINCT ", 16).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "DISTINCT after SELECT should suggest columns"
    );
}

#[test]
fn test_detect_select_all() {
    let result = detect_context("SELECT ALL ", 11).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "ALL after SELECT should suggest columns"
    );
}

// ---- Comments ----

#[test]
fn test_detect_inline_comment_does_not_leak() {
    let result = detect_context("SELECT * FROM users /* find all users */ WHERE ", 49).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

#[test]
fn test_detect_block_comment_no_leak_to_where() {
    let result = detect_context("SELECT * FROM users /* comment */ WHERE ", 41).unwrap();
    assert_eq!(result.context_type, ContextType::Column);
}

// ---- Extract tables ----

#[test]
fn test_extract_tables_simple() {
    let result = detect_context("SELECT * FROM users WHERE ", 25).unwrap();
    assert!(result.tables.iter().any(|t| t.name == "users"));
}

#[test]
fn test_extract_tables_with_alias() {
    let result = detect_context("SELECT * FROM users u WHERE ", 27).unwrap();
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "users" && t.alias == Some("u".into())));
}

#[test]
fn test_extract_tables_join_with_aliases() {
    let result = detect_context(
        "SELECT * FROM users u JOIN posts p ON u.id = p.id WHERE ",
        50,
    )
    .unwrap();
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "users" && t.alias == Some("u".into())));
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "posts" && t.alias == Some("p".into())));
}

#[test]
fn test_extract_tables_no_leak_from_subquery() {
    let result = detect_context(
        "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders) AND ",
        62,
    )
    .unwrap();
    assert!(result.tables.iter().any(|t| t.name == "users"));
    assert!(
        !result.tables.iter().any(|t| t.name == "orders"),
        "orders is inside a subquery and should not leak"
    );
}

#[test]
fn test_extract_tables_cross_join() {
    let result = detect_context("SELECT * FROM users CROSS JOIN posts WHERE ", 39).unwrap();
    assert!(result.tables.iter().any(|t| t.name == "users"));
    assert!(result.tables.iter().any(|t| t.name == "posts"));
}

#[test]
fn test_extract_schema_qualified_table() {
    let result = detect_context("SELECT * FROM public.users WHERE ", 30).unwrap();
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "users" && t.schema == Some("public".into())));
}

#[test]
fn test_extract_schema_alias() {
    let result = detect_context("SELECT * FROM public.users u WHERE ", 29).unwrap();
    assert!(
        result.tables.iter().any(|t| {
            t.name == "users" && t.schema == Some("public".into()) && t.alias == Some("u".into())
        }),
        "users with schema public and alias u should be found"
    );
}

#[test]
fn test_extract_schema_join() {
    let result = detect_context(
        "SELECT * FROM public.users u JOIN blog.posts p ON u.id = p.user_id WHERE ",
        60,
    )
    .unwrap();
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "users" && t.schema == Some("public".into())));
    assert!(result
        .tables
        .iter()
        .any(|t| t.name == "posts" && t.schema == Some("blog".into())));
}

#[test]
fn test_extract_multi_join_with_schema() {
    let result = detect_context(
        "SELECT * FROM public.users u JOIN posts p ON u.id = p.author_id JOIN comments c ON p.id = c.post_id WHERE ",
        98,
    ).unwrap();
    assert!(result.tables.iter().any(|t| t.name == "users"));
    assert!(result.tables.iter().any(|t| t.name == "posts"));
    assert!(result.tables.iter().any(|t| t.name == "comments"));
}

#[test]
fn test_extract_three_level_name() {
    let result = detect_context("SELECT * FROM mydb.public.users WHERE ", 34).unwrap();
    assert!(
        result
            .tables
            .iter()
            .any(|t| { t.name == "users" && t.schema == Some("public".into()) }),
        "three-level name should extract schema=public, table=users"
    );
}

#[test]
fn test_extract_three_level_name_with_alias() {
    let result = detect_context("SELECT * FROM mydb.public.users u WHERE ", 33).unwrap();
    assert!(
        result.tables.iter().any(|t| {
            t.name == "users" && t.schema == Some("public".into()) && t.alias == Some("u".into())
        }),
        "three-level name with alias should extract name, schema, and alias"
    );
}

#[test]
fn test_extract_three_level_name_with_as_alias() {
    let result = detect_context("SELECT * FROM mydb.public.users AS u WHERE ", 36).unwrap();
    assert!(
        result.tables.iter().any(|t| {
            t.name == "users" && t.schema == Some("public".into()) && t.alias == Some("u".into())
        }),
        "three-level name with AS alias should work"
    );
}

#[test]
fn test_extract_table_with_as_alias_no_schema() {
    let result = detect_context("SELECT * FROM users AS u WHERE ", 28).unwrap();
    assert!(
        result
            .tables
            .iter()
            .any(|t| { t.name == "users" && t.alias == Some("u".into()) }),
        "table with AS alias and no schema should extract alias"
    );
}

#[test]
fn test_extract_join_with_schema_and_alias() {
    let result = detect_context(
        "SELECT * FROM public.users AS u JOIN public.posts AS p ON u.id = p.user_id WHERE ",
        68,
    )
    .unwrap();
    assert!(result.tables.iter().any(|t| {
        t.name == "users" && t.schema == Some("public".into()) && t.alias == Some("u".into())
    }));
    assert!(result.tables.iter().any(|t| {
        t.name == "posts" && t.schema == Some("public".into()) && t.alias == Some("p".into())
    }));
}

#[test]
fn test_extract_natural_join() {
    let result = detect_context("SELECT * FROM users NATURAL JOIN posts WHERE ", 40).unwrap();
    assert!(result.tables.iter().any(|t| t.name == "users"));
    assert!(result.tables.iter().any(|t| t.name == "posts"));
}

#[test]
fn test_extract_table_with_dash() {
    let result = detect_context("SELECT * FROM posts-web_vitals WHERE ", 31).unwrap();
    assert!(result.tables.iter().any(|t| t.name == "posts-web_vitals"));
}

// ---- Statement span ----

#[test]
fn test_find_statement_span_simple() {
    let lines = vec!["SELECT 1;", "SELECT 2;"];
    let span = find_statement_span(&lines, 0);
    assert_eq!(span, Some((0, 0)));
    let span2 = find_statement_span(&lines, 1);
    assert_eq!(span2, Some((1, 1)));
}

#[test]
fn test_find_statement_span_with_semicolon_in_string() {
    let lines = vec!["SELECT 'hello;world'"];
    let span = find_statement_span(&lines, 0);
    assert_eq!(span, Some((0, 0)));
}

#[test]
fn test_find_statement_span_semicolon_in_dollar_string() {
    let lines = vec!["SELECT $$abc;def$$;", "SELECT 2"];
    let span = find_statement_span(&lines, 1);
    assert_ne!(span, None);
}

#[test]
fn test_find_statement_span_multi_statement_on_same_line() {
    let lines = vec!["SELECT 1; SELECT 2;"];
    let span = find_statement_span(&lines, 0);
    assert_eq!(span, Some((0, 0)));
}

#[test]
fn test_tables_isolated_to_current_statement() {
    let sql = "select 1 from posts; select 2 from authors";
    let result = detect_context(sql, 35).unwrap();
    assert!(
        result.tables.iter().any(|t| t.name == "authors"),
        "should have authors"
    );
    assert!(
        !result.tables.iter().any(|t| t.name == "posts"),
        "should NOT have posts from prior stmt"
    );
}

#[test]
fn test_modify_column_context() {
    let sql = "ALTER TABLE posts MODIFY COLUMN ";
    let result = detect_context(sql, 32).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "MODIFY COLUMN should suggest column names"
    );
}

#[test]
fn test_modify_column_after_name_context() {
    let sql = "ALTER TABLE posts MODIFY COLUMN age ";
    let result = detect_context(sql, 36).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::DataType,
        "MODIFY COLUMN colname should suggest data types"
    );
}

// ---- Lua offset regression (cursor_line +1 bug) ----

#[test]
fn test_detect_select_then_from_with_cursor_on_ws() {
    let sql = "SELECT  FROM authors WHERE id > 1;";
    let result = detect_context(sql, 7).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "cursor on whitespace after SELECT, before FROM → Column"
    );
    assert!(
        result.tables.iter().any(|t| t.name == "authors"),
        "tables extracted from the FROM clause: {:?}",
        result.tables,
    );
}

#[test]
fn test_detect_select_then_from_with_cursor_on_from() {
    let sql = "SELECT  FROM authors WHERE id > 1;";
    let result = detect_context(sql, 8).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Table,
        "cursor on FROM itself → Table context"
    );
}

#[test]
fn test_detect_offset_past_end_falls_to_last_semantic_token() {
    let sql = "SELECT  FROM authors WHERE id > 1;";
    let result = detect_context(sql, 99).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Keyword,
        "out-of-bounds offset should return Keyword, not panic",
    );
}

#[test]
fn test_detect_with_directives_like_lua_offset() {
    let sql = "-- @connection my-blog\n-- @database blog\n\nSELECT  FROM authors WHERE id > 1;";
    let result = detect_context(sql, 49).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "full-directive reproduction: cursor on whitespace after SELECT → Column"
    );
    assert!(
        result.tables.iter().any(|t| t.name == "authors"),
        "authors table extracted despite directives: {:?}",
        result.tables,
    );
}

#[test]
fn test_where_with_prefix_returns_column() {
    let sql = "SELECT * FROM authors WHERE e";
    let result = detect_context(sql, 29).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "typing column prefix after WHERE → Column, got {:?}",
        result.context_type,
    );
}

#[test]
fn test_tables_from_all_statements_without_semicolon() {
    // With semantic boundary detection, UPDATE and SELECT are separate
    // statements. Cursor is in the UPDATE statement — only its table
    // refs should be in scope.
    let sql =
        "UPDATE authors Set \n\nSELECT * from posts p left JOIN authors a on p.author_id = a.id;";
    let result = detect_context(sql, 19).unwrap();
    assert_eq!(
        result.context_type,
        ContextType::Column,
        "should be column context after SET"
    );
    assert_eq!(
        result.tables.len(),
        1,
        "should have only UPDATE's table ref, got {:?}",
        result.tables
    );
    assert!(
        result
            .tables
            .iter()
            .any(|t| t.name == "authors" && t.alias.is_none()),
        "authors without alias from UPDATE should be present"
    );
}
