use super::tables::parse_table_ref;
use super::tokenizer::{
    is_known_keyword, is_set_operator, is_table_keyword, kw_eq, skip_forward, Token, TokenKind,
};
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

    /// Tables declared here that are not `WITH` names. A set-operation arm
    /// hides this many of its parent's leading tables: the previous arm's `FROM`
    /// list is out of scope, while the statement's CTEs stay in it.
    fn non_cte_tables(&self) -> usize {
        self.tables
            .iter()
            .filter(|t| !self.ctes.iter().any(|c| c.name == t.name))
            .count()
    }
}

/// A query block's table scope.
///
/// Frames are never removed from the list, only de-activated: a cursor that was
/// recorded inside a subquery or an earlier `UNION` arm still resolves to the
/// frame it belonged to after the scan has moved on.
struct Frame {
    parent: usize,
    /// How many leading tables of `parent` to skip when walking up. A set
    /// operation's arm uses it to hide the tables the previous arm declared —
    /// they are the ones already in the parent when the arm starts, and nothing
    /// registered later (a `WITH` name is appended after them).
    hidden_prefix: usize,
    scope: QueryScope,
}

/// Resolve the tables visible at `cursor_idx` (an index into `tokens`), or the
/// statement's own top-level tables when there is no cursor.
///
/// Two kinds of boundary get a frame: parentheses (a subquery's `FROM` list
/// stays out of the enclosing query, while the enclosing query stays visible to
/// it for correlated references) and the arms of a set operation, which see
/// neither each other nor anything the scan put away behind them.
pub(crate) fn resolve_scope_at(
    tokens: &[Token],
    sql: &str,
    cursor_idx: Option<usize>,
) -> QueryScope {
    let mut frames = vec![Frame {
        parent: 0,
        hidden_prefix: 0,
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
                    hidden_prefix: 0,
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

                // The arm after a set operator starts with a clean table list:
                // keep walking up for correlated references, but hide the
                // tables the previous arm declared in the parent.
                if is_set_operator(&kw_lower) {
                    let hidden = frames[top].scope.non_cte_tables();
                    frames.push(Frame {
                        parent: top,
                        hidden_prefix: hidden,
                        scope: QueryScope::empty(),
                    });
                    top = frames.len() - 1;
                }

                let scope = &mut frames[top].scope;

                if kw_eq(kw_text, "with") {
                    extract_cte_names(tokens, i, sql, scope);
                }

                if is_table_keyword(&kw_lower) {
                    if let Some(next) = skip_forward(tokens, i) {
                        register_table_list(tokens, next, sql, scope);
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

    let mut visible = QueryScope::empty();
    match cursor_idx {
        // No cursor: the statement's own frame, which is what the
        // subquery-insensitive table listing wants.
        None => visible = frames.remove(0).scope,
        // Innermost frame first: the completion menu should lead with what the
        // cursor's own query block selects from.
        Some(_) => {
            let mut at = cursor_frame;
            // How many leading tables of the frame we are about to enter its
            // child asked us to hide.
            let mut skip = 0usize;
            loop {
                let frame = &frames[at];
                for table in frame.scope.tables.iter().skip(skip) {
                    visible.add_table(table.clone());
                }
                for cte in &frame.scope.ctes {
                    if !visible.ctes.iter().any(|c| c.name == cte.name) {
                        visible.ctes.push(CteRef {
                            name: cte.name.clone(),
                        });
                    }
                }
                skip = frame.hidden_prefix;
                let parent = frame.parent;
                if parent == at {
                    break;
                }
                at = parent;
            }
        }
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
                            i = matching_rparen(tokens, next).unwrap_or(tokens.len());
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
                            check = matching_rparen(tokens, check).unwrap_or(tokens.len());
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
                                i = matching_rparen(tokens, body_start).unwrap_or(tokens.len());
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

/// Index of the next non-trivia token at or after `i` — `i` itself when it is
/// already significant.  Callers that want the token *following* `i` must pass
/// `i + 1`, otherwise they re-read the token they just consumed.
fn next_significant(tokens: &[Token], mut i: usize) -> Option<usize> {
    while i < tokens.len() {
        if !matches!(
            tokens[i].kind,
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
        ) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `LATERAL` and `ONLY` qualify the reference that follows them; they are not
/// table names themselves.
fn is_table_modifier(tokens: &[Token], i: usize, sql: &str) -> bool {
    let tok = &tokens[i];
    if !matches!(
        tok.kind,
        TokenKind::Ident | TokenKind::QuotedIdent | TokenKind::Keyword
    ) {
        return false;
    }
    let text = tok.display_text(sql);
    if !(text.eq_ignore_ascii_case("lateral") || text.eq_ignore_ascii_case("only")) {
        return false;
    }
    // `FROM only, x` / `FROM only WHERE …` — a bare name, not a modifier.
    matches!(
        next_significant(tokens, i + 1),
        Some(j) if matches!(
            tokens[j].kind,
            TokenKind::LParen | TokenKind::Ident | TokenKind::QuotedIdent
        )
    )
}

/// Register the comma-separated table list that starts at `start`: `a`,
/// `a AS x`, `a, b`, `a, (SELECT …) b`. Stops at the first token that cannot
/// continue the list.
///
/// This only records names: the caller's scan keeps its own paren frames, so
/// nothing consumed here advances it.
fn register_table_list(tokens: &[Token], start: usize, sql: &str, scope: &mut QueryScope) {
    let mut i = start;
    loop {
        i = match next_significant(tokens, i) {
            Some(x) => x,
            None => return,
        };
        if is_table_modifier(tokens, i, sql) {
            i = match next_significant(tokens, i + 1) {
                Some(x) => x,
                None => return,
            };
        }

        if tokens[i].kind == TokenKind::LParen {
            // A parenthesised element is a derived table; only its alias names
            // a relation the query block can reference.
            if let Some(name) = extract_derived_table_alias(tokens, i, sql) {
                scope.add_virtual_table(&name);
            }
            i = match matching_rparen(tokens, i) {
                Some(after) => after,
                None => return,
            };
        } else if matches!(
            tokens[i].kind,
            TokenKind::Ident | TokenKind::QuotedIdent | TokenKind::Keyword
        ) {
            // `gen_series(…) AS g`: the alias after the argument list is
            // what the rest of the block refers to.
            let args =
                next_significant(tokens, i + 1).filter(|&x| tokens[x].kind == TokenKind::LParen);
            let (schema, table_name, alias, consumed) = parse_table_ref(tokens, i, sql);
            if table_name.is_empty() {
                return;
            }
            scope.add_table(TableRef {
                name: table_name.to_string(),
                alias: alias.map(|s| s.to_string()),
                schema: schema.map(|s| s.to_string()),
            });
            match args {
                Some(lp) => {
                    if let Some(name) = extract_derived_table_alias(tokens, lp, sql) {
                        scope.add_virtual_table(&name);
                    }
                    i = match matching_rparen(tokens, lp) {
                        Some(after) => after,
                        None => return,
                    };
                }
                None => i += consumed,
            }
        } else {
            return;
        }

        i = skip_alias_tail(tokens, i, sql);
        match next_significant(tokens, i) {
            Some(x) if tokens[x].kind == TokenKind::Comma => i = x + 1,
            _ => return,
        }
    }
}

/// Step over the alias that follows a parenthesised element (`AS d`, or a bare
/// `d`) so the list can continue to the next comma. Anything else that comes
/// next (`WHERE`, `ON`, another `JOIN`) belongs to the following clause.
fn skip_alias_tail(tokens: &[Token], i: usize, sql: &str) -> usize {
    let first = match next_significant(tokens, i) {
        Some(x) => x,
        None => return i,
    };
    if tokens[first].kind == TokenKind::Keyword && kw_eq(tokens[first].text(sql), "as") {
        return match next_significant(tokens, first + 1) {
            Some(name)
                if matches!(tokens[name].kind, TokenKind::Ident | TokenKind::QuotedIdent) =>
            {
                name + 1
            }
            _ => first,
        };
    }
    if matches!(
        tokens[first].kind,
        TokenKind::Ident | TokenKind::QuotedIdent
    ) {
        return first + 1;
    }
    first
}

/// Index just past the `)` that closes the `(` at `lp_idx`, or `None` when the
/// parenthesis is never closed (half-typed SQL).
fn matching_rparen(tokens: &[Token], lp_idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut j = lp_idx;
    while j < tokens.len() {
        match tokens[j].kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

fn extract_derived_table_alias(tokens: &[Token], lp_idx: usize, sql: &str) -> Option<String> {
    let after_close = matching_rparen(tokens, lp_idx)?;

    if let Some(alias_start) = skip_forward(tokens, after_close - 1) {
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
    fn cursor_after_set_operator_leaves_the_previous_arm() {
        let scope =
            scope_at_cursor("SELECT a FROM first_t UNION ALL SELECT b FROM second_t WHERE ▮");
        let names = table_names(&scope);
        assert!(names.contains(&"second_t"), "got {names:?}");
        assert!(
            !names.contains(&"first_t"),
            "the finished arm leaked: {names:?}"
        );
    }

    #[test]
    fn cte_names_survive_a_set_operator() {
        let scope = scope_at_cursor("WITH c AS (SELECT 1) SELECT * FROM c UNION SELECT 2 WHERE ▮");
        let names = table_names(&scope);
        assert!(
            names.contains(&"c"),
            "a WITH name belongs to the whole statement: {names:?}"
        );
    }

    #[test]
    fn comma_separated_from_list_registers_every_element() {
        let scope = scope_at_cursor("SELECT * FROM users, logs WHERE ▮");
        let names = table_names(&scope);
        assert!(names.contains(&"users"), "got {names:?}");
        assert!(
            names.contains(&"logs"),
            "a comma joins another table into the same block: {names:?}"
        );

        let scope = scope_at_cursor("SELECT * FROM users u, logs l WHERE ▮");
        assert!(
            scope
                .tables
                .iter()
                .any(|t| t.name == "logs" && t.alias.as_deref() == Some("l")),
            "got {:?}",
            table_names(&scope)
        );
    }

    #[test]
    fn comma_list_keeps_derived_and_qualified_elements() {
        let scope = scope_at_cursor("SELECT * FROM users, (SELECT 1) AS d, public.logs WHERE ▮");
        let names = table_names(&scope);
        assert!(names.contains(&"users"), "got {names:?}");
        assert!(names.contains(&"d"), "got {names:?}");
        assert!(names.contains(&"logs"), "got {names:?}");
    }

    #[test]
    fn lateral_and_only_qualify_instead_of_naming_a_table() {
        let scope =
            scope_at_cursor("SELECT * FROM users JOIN LATERAL (SELECT * FROM orders WHERE ▮) o");
        let names = table_names(&scope);
        assert!(names.contains(&"orders"), "got {names:?}");
        assert!(
            !names.contains(&"LATERAL"),
            "LATERAL is a modifier, not a table: {names:?}"
        );

        let scope = scope_at_cursor("SELECT * FROM ONLY tbl WHERE ▮");
        let names = table_names(&scope);
        assert!(names.contains(&"tbl"), "got {names:?}");
        assert!(!names.contains(&"ONLY"), "got {names:?}");
    }

    #[test]
    fn bare_only_remains_a_table_name() {
        let scope = scope_at_cursor("SELECT * FROM only WHERE ▮");
        let names = table_names(&scope);
        assert!(names.contains(&"only"), "got {names:?}");
    }

    #[test]
    fn table_function_registers_its_alias() {
        let scope = scope_at_cursor("SELECT * FROM generate_series(1, 10) AS g WHERE ▮");
        let names = table_names(&scope);
        assert!(
            names.contains(&"g"),
            "the block refers to the function output by its alias: {names:?}"
        );
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
