use super::tables::parse_table_ref;
use super::tokenizer::{is_known_keyword, is_table_keyword, kw_eq, skip_forward, Token, TokenKind};
use super::TableRef;

pub(crate) struct CteRef {
    pub name: String,
}

pub(crate) struct QueryScope {
    pub tables: Vec<TableRef>,
    pub ctes: Vec<CteRef>,
}

impl QueryScope {
    pub(crate) fn empty() -> Self {
        QueryScope {
            tables: vec![],
            ctes: vec![],
        }
    }

    fn add_table(&mut self, table: TableRef) {
        if !self
            .tables
            .iter()
            .any(|t| t.name == table.name && t.alias == table.alias && t.schema == table.schema)
        {
            self.tables.push(table);
        }
    }

    fn has_table_named(&self, name: &str) -> bool {
        self.tables.iter().any(|t| t.name == name)
    }

    fn add_virtual_table(&mut self, name: &str) {
        if !self.has_table_named(name) {
            self.tables.push(TableRef {
                name: name.to_string(),
                alias: None,
                schema: None,
            });
        }
    }
}

/// One paren level of query scoping. Frames are never removed from `frames`
/// (sibling subqueries must not share a frame), only de-activated.
struct Frame {
    parent: usize,
    scope: QueryScope,
}

/// Resolve the tables visible at `cursor_idx` (an index into `tokens`).
///
/// Each paren level gets its own frame. A subquery's `FROM` list stays out of
/// the enclosing query, but the enclosing frames stay visible to the subquery
/// so correlated references (`WHERE id IN (SELECT … FROM orders WHERE |)`)
/// still offer both `orders` and the outer table. Without a cursor the result
/// is the top-level frame only, which is what the statement-boundary callers
/// want.
pub(crate) fn resolve_scope_at(
    tokens: &[Token],
    sql: &str,
    cursor_idx: Option<usize>,
) -> QueryScope {
    let mut frames = vec![Frame {
        parent: 0,
        scope: QueryScope::empty(),
    }];
    let mut top = 0usize;
    let mut cursor_frame = 0usize;
    let mut i = 0;

    while i < tokens.len() {
        if Some(i) == cursor_idx {
            cursor_frame = top;
        }
        let t = &tokens[i];
        match t.kind {
            TokenKind::LParen => {
                frames.push(Frame {
                    parent: top,
                    scope: QueryScope::empty(),
                });
                top = frames.len() - 1;
            }
            // An unmatched ')' at the top level is a typo in incomplete SQL,
            // not a scope exit — the wildcard arm ignores it rather than
            // underflowing the frame stack.
            TokenKind::RParen if top > 0 => {
                top = frames[top].parent;
            }
            TokenKind::Keyword => {
                let kw_text = t.text(sql);
                let kw_lower = kw_text.to_ascii_lowercase();
                let scope = &mut frames[top].scope;

                if kw_eq(kw_text, "with") {
                    extract_cte_names(tokens, i, sql, scope);
                }

                if is_table_keyword(&kw_lower) {
                    if let Some(next) = skip_forward(tokens, i) {
                        if tokens[next].kind == TokenKind::LParen {
                            if let Some(name) = extract_derived_table_alias(tokens, next, sql) {
                                scope.add_virtual_table(&name);
                            }
                        } else {
                            let (schema, table_name, alias, _consumed) =
                                parse_table_ref(tokens, next, sql);
                            if !table_name.is_empty() {
                                scope.add_table(TableRef {
                                    name: table_name.to_string(),
                                    alias: alias.map(|s| s.to_string()),
                                    schema: schema.map(|s| s.to_string()),
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }

    // CTE names are referenceable as tables within the block that declares them.
    for frame in &mut frames {
        let cte_names: Vec<String> = frame.scope.ctes.iter().map(|c| c.name.clone()).collect();
        for name in &cte_names {
            frame.scope.add_virtual_table(name);
        }
    }

    if cursor_frame == 0 {
        return frames.remove(0).scope;
    }

    // Innermost frame first: the completion menu should lead with what the
    // cursor's own query block selects from.
    let mut visible = QueryScope::empty();
    let mut at = cursor_frame;
    loop {
        for table in &frames[at].scope.tables {
            visible.add_table(table.clone());
        }
        for cte in &frames[at].scope.ctes {
            if !visible.ctes.iter().any(|c| c.name == cte.name) {
                visible.ctes.push(CteRef {
                    name: cte.name.clone(),
                });
            }
        }
        if at == 0 {
            break;
        }
        at = frames[at].parent;
    }
    visible
}

fn extract_cte_names(tokens: &[Token], with_idx: usize, sql: &str, scope: &mut QueryScope) {
    let mut i = with_idx + 1;
    let mut found_cte = false;

    while i < tokens.len() {
        match tokens[i].kind {
            TokenKind::Keyword if found_cte => {
                let kw = tokens[i].text(sql).to_ascii_lowercase();
                if matches!(
                    kw.as_str(),
                    "select"
                        | "update"
                        | "delete"
                        | "insert"
                        | "create"
                        | "alter"
                        | "drop"
                        | "truncate"
                        | "explain"
                        | "show"
                ) {
                    break;
                }
                if kw_eq(tokens[i].text(sql), "as") {
                    if let Some(next) = skip_forward(tokens, i) {
                        if tokens[next].kind == TokenKind::LParen {
                            let mut depth = 1;
                            let mut j = next + 1;
                            while j < tokens.len() && depth > 0 {
                                match tokens[j].kind {
                                    TokenKind::LParen => depth += 1,
                                    TokenKind::RParen => depth -= 1,
                                    _ => {}
                                }
                                j += 1;
                            }
                            i = j;
                            continue;
                        }
                    }
                }
            }
            TokenKind::Ident | TokenKind::QuotedIdent => {
                let mut check = i + 1;
                while check < tokens.len() {
                    match tokens[check].kind {
                        TokenKind::Whitespace
                        | TokenKind::LineComment
                        | TokenKind::BlockComment => {
                            check += 1;
                        }
                        TokenKind::LParen => {
                            let mut depth = 1;
                            let mut j = check + 1;
                            while j < tokens.len() && depth > 0 {
                                match tokens[j].kind {
                                    TokenKind::LParen => depth += 1,
                                    TokenKind::RParen => depth -= 1,
                                    _ => {}
                                }
                                j += 1;
                            }
                            check = j;
                        }
                        _ => break,
                    }
                }
                if check < tokens.len() {
                    let tok = &tokens[check];
                    if tok.kind == TokenKind::Keyword && kw_eq(tok.text(sql), "as") {
                        let name = tokens[i].display_text(sql).to_string();
                        scope.ctes.push(CteRef { name });
                        found_cte = true;
                        if let Some(body_start) = skip_forward(tokens, check) {
                            if tokens[body_start].kind == TokenKind::LParen {
                                let mut depth = 1;
                                let mut j = body_start + 1;
                                while j < tokens.len() && depth > 0 {
                                    match tokens[j].kind {
                                        TokenKind::LParen => depth += 1,
                                        TokenKind::RParen => depth -= 1,
                                        _ => {}
                                    }
                                    j += 1;
                                }
                                i = j;
                                continue;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn extract_derived_table_alias(tokens: &[Token], lp_idx: usize, sql: &str) -> Option<String> {
    let mut depth = 1;
    let mut j = lp_idx + 1;
    while j < tokens.len() && depth > 0 {
        match tokens[j].kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    if depth != 0 {
        return None;
    }

    if let Some(alias_start) = skip_forward(tokens, j - 1) {
        let alias_tok = &tokens[alias_start];
        if alias_tok.kind == TokenKind::Keyword && kw_eq(alias_tok.text(sql), "as") {
            if let Some(name_idx) = skip_forward(tokens, alias_start) {
                let name_tok = &tokens[name_idx];
                if matches!(
                    name_tok.kind,
                    TokenKind::Ident | TokenKind::QuotedIdent | TokenKind::Keyword
                ) {
                    return Some(name_tok.display_text(sql).to_string());
                }
            }
        } else if matches!(alias_tok.kind, TokenKind::Ident | TokenKind::QuotedIdent) {
            let text = alias_tok.display_text(sql);
            if !is_known_keyword(text) {
                return Some(text.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_context::tokenizer::{find_token_at_offset, tokenize};

    /// Resolve the scope at a `▮` cursor marker, the way completion does.
    fn scope_at_cursor(marked: &str) -> QueryScope {
        let offset = marked.find('▮').expect("▮ marks the cursor");
        let sql = marked.replace('▮', "");
        let tokens = tokenize(&sql);
        let raw = find_token_at_offset(&tokens, offset).unwrap_or(0);
        let cursor_idx = if raw + 1 < tokens.len() && offset > tokens[raw].end {
            raw + 1
        } else {
            raw
        };
        resolve_scope_at(&tokens, &sql, Some(cursor_idx))
    }

    fn table_names(scope: &QueryScope) -> Vec<&str> {
        scope.tables.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn cursor_inside_subquery_sees_its_own_from() {
        let scope =
            scope_at_cursor("SELECT * FROM users WHERE id IN (SELECT uid FROM orders ▮) = 1");
        let names = table_names(&scope);
        assert!(
            names.contains(&"orders"),
            "the subquery's own FROM must be visible, got {names:?}"
        );
        assert!(
            names.contains(&"users"),
            "the enclosing query stays visible (correlated reference), got {names:?}"
        );
        assert_eq!(
            names.first().copied(),
            Some("orders"),
            "innermost frame first, got {names:?}"
        );
    }

    #[test]
    fn cursor_outside_subquery_still_hides_its_from() {
        let scope =
            scope_at_cursor("SELECT * FROM users WHERE id IN (SELECT uid FROM orders) AND ▮");
        let names = table_names(&scope);
        assert!(names.contains(&"users"), "got {names:?}");
        assert!(
            !names.contains(&"orders"),
            "a finished subquery must not leak, got {names:?}"
        );
    }

    #[test]
    fn sibling_subqueries_do_not_share_a_frame() {
        let scope = scope_at_cursor(
            "SELECT * FROM users WHERE a IN (SELECT x FROM one) AND b IN (SELECT y FROM two ▮)",
        );
        let names = table_names(&scope);
        assert!(names.contains(&"two"), "got {names:?}");
        assert!(
            !names.contains(&"one"),
            "the closed sibling must not leak, got {names:?}"
        );
    }

    #[test]
    fn cursor_in_derived_table_sees_the_inner_from() {
        let scope = scope_at_cursor("SELECT * FROM (SELECT * FROM items WHERE ▮) sub");
        let names = table_names(&scope);
        assert!(names.contains(&"items"), "got {names:?}");
    }

    #[test]
    fn cursor_in_cte_body_sees_the_cte_source() {
        let scope = scope_at_cursor("WITH o AS (SELECT * FROM orders WHERE ▮) SELECT * FROM inv");
        let names = table_names(&scope);
        assert!(names.contains(&"orders"), "got {names:?}");
    }

    #[test]
    fn unmatched_rparen_does_not_pop_the_top_level_frame() {
        // A stray ')' in half-typed SQL used to drive the paren counter
        // negative, so the following `FROM` was read as nested and dropped.
        let scope = scope_at_cursor("SELECT * FROM users WHERE ) ▮ x");
        assert!(
            table_names(&scope).contains(&"users"),
            "got {:?}",
            table_names(&scope)
        );

        let scope = scope_at_cursor("SELECT ) * FROM users WHERE ▮");
        assert!(
            table_names(&scope).contains(&"users"),
            "a FROM after a stray ')' still belongs to the statement, got {:?}",
            table_names(&scope)
        );
    }

    #[test]
    fn test_resolve_scope_empty() {
        let scope = resolve_scope_at(&[], "", None);
        assert!(scope.tables.is_empty());
    }

    #[test]
    fn test_resolve_scope_simple_from() {
        let sql = "SELECT * FROM users";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert_eq!(scope.tables.len(), 1);
        assert_eq!(scope.tables[0].name, "users");
    }

    #[test]
    fn test_resolve_scope_join() {
        let sql = "SELECT * FROM users u JOIN posts p ON u.id = p.id";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert_eq!(scope.tables.len(), 2);
        assert!(scope
            .tables
            .iter()
            .any(|t| t.name == "users" && t.alias == Some("u".into())));
        assert!(scope
            .tables
            .iter()
            .any(|t| t.name == "posts" && t.alias == Some("p".into())));
    }

    #[test]
    fn test_resolve_scope_subquery_not_leaked() {
        let sql = "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders)";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert!(scope.tables.iter().any(|t| t.name == "users"));
        assert!(!scope.tables.iter().any(|t| t.name == "orders"));
    }

    #[test]
    fn test_resolve_scope_cte() {
        let sql = "WITH cte AS (SELECT * FROM users) SELECT * FROM cte";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert_eq!(scope.ctes.len(), 1);
        assert_eq!(scope.ctes[0].name, "cte");
        assert!(scope.tables.iter().any(|t| t.name == "cte"));
        assert!(!scope.tables.iter().any(|t| t.name == "users"));
    }

    #[test]
    fn test_resolve_scope_derived_table_alias() {
        let sql = "SELECT * FROM (SELECT * FROM items) AS sub";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert!(scope.tables.iter().any(|t| t.name == "sub"));
        assert!(!scope.tables.iter().any(|t| t.name == "items"));
    }

    #[test]
    fn test_resolve_scope_derived_table_bare_alias() {
        let sql = "SELECT * FROM (SELECT 1) sub";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert!(scope.tables.iter().any(|t| t.name == "sub"));
    }

    #[test]
    fn test_resolve_scope_schema_table() {
        let sql = "SELECT * FROM public.users";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert!(scope
            .tables
            .iter()
            .any(|t| t.name == "users" && t.schema == Some("public".into())));
    }

    #[test]
    fn test_resolve_scope_update() {
        let sql = "UPDATE users SET name = 'x'";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert!(scope.tables.iter().any(|t| t.name == "users"));
    }

    #[test]
    fn test_resolve_scope_nested_derived_table() {
        let sql = "SELECT * FROM (SELECT * FROM (SELECT * FROM deep) AS mid) AS outer WHERE ";
        let tokens = tokenize(sql);
        let scope = resolve_scope_at(&tokens, sql, None);
        assert!(
            scope.tables.iter().any(|t| t.name == "outer"),
            "outer should be visible, got tables: {:?}",
            scope.tables
        );
        assert!(
            !scope.tables.iter().any(|t| t.name == "deep"),
            "deep should not leak, got tables: {:?}",
            scope.tables
        );
        assert!(
            !scope.tables.iter().any(|t| t.name == "mid"),
            "mid should not leak (inner alias), got tables: {:?}",
            scope.tables
        );
    }
}
