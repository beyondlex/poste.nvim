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

pub(crate) fn resolve_scope(tokens: &[Token], sql: &str) -> QueryScope {
    let mut scope = QueryScope::empty();
    let mut i = 0;
    let mut paren_depth = 0i32;

    while i < tokens.len() {
        let t = &tokens[i];
        match t.kind {
            TokenKind::LParen => paren_depth += 1,
            TokenKind::RParen => paren_depth -= 1,
            TokenKind::Keyword if paren_depth == 0 => {
                let kw_text = t.text(sql);
                let kw_lower = kw_text.to_ascii_lowercase();

                if kw_eq(kw_text, "with") {
                    extract_cte_names(tokens, i, sql, &mut scope);
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

    let cte_names: Vec<String> = scope.ctes.iter().map(|c| c.name.clone()).collect();
    for name in &cte_names {
        if !scope.has_table_named(name) {
            scope.add_virtual_table(name);
        }
    }

    scope
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
    use crate::sql_context::tokenizer::tokenize;

    #[test]
    fn test_resolve_scope_empty() {
        let scope = resolve_scope(&[], "");
        assert!(scope.tables.is_empty());
    }

    #[test]
    fn test_resolve_scope_simple_from() {
        let sql = "SELECT * FROM users";
        let tokens = tokenize(sql);
        let scope = resolve_scope(&tokens, sql);
        assert_eq!(scope.tables.len(), 1);
        assert_eq!(scope.tables[0].name, "users");
    }

    #[test]
    fn test_resolve_scope_join() {
        let sql = "SELECT * FROM users u JOIN posts p ON u.id = p.id";
        let tokens = tokenize(sql);
        let scope = resolve_scope(&tokens, sql);
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
        let scope = resolve_scope(&tokens, sql);
        assert!(scope.tables.iter().any(|t| t.name == "users"));
        assert!(!scope.tables.iter().any(|t| t.name == "orders"));
    }

    #[test]
    fn test_resolve_scope_cte() {
        let sql = "WITH cte AS (SELECT * FROM users) SELECT * FROM cte";
        let tokens = tokenize(sql);
        let scope = resolve_scope(&tokens, sql);
        assert_eq!(scope.ctes.len(), 1);
        assert_eq!(scope.ctes[0].name, "cte");
        assert!(scope.tables.iter().any(|t| t.name == "cte"));
        assert!(!scope.tables.iter().any(|t| t.name == "users"));
    }

    #[test]
    fn test_resolve_scope_derived_table_alias() {
        let sql = "SELECT * FROM (SELECT * FROM items) AS sub";
        let tokens = tokenize(sql);
        let scope = resolve_scope(&tokens, sql);
        assert!(scope.tables.iter().any(|t| t.name == "sub"));
        assert!(!scope.tables.iter().any(|t| t.name == "items"));
    }

    #[test]
    fn test_resolve_scope_derived_table_bare_alias() {
        let sql = "SELECT * FROM (SELECT 1) sub";
        let tokens = tokenize(sql);
        let scope = resolve_scope(&tokens, sql);
        assert!(scope.tables.iter().any(|t| t.name == "sub"));
    }

    #[test]
    fn test_resolve_scope_schema_table() {
        let sql = "SELECT * FROM public.users";
        let tokens = tokenize(sql);
        let scope = resolve_scope(&tokens, sql);
        assert!(scope
            .tables
            .iter()
            .any(|t| t.name == "users" && t.schema == Some("public".into())));
    }

    #[test]
    fn test_resolve_scope_update() {
        let sql = "UPDATE users SET name = 'x'";
        let tokens = tokenize(sql);
        let scope = resolve_scope(&tokens, sql);
        assert!(scope.tables.iter().any(|t| t.name == "users"));
    }

    #[test]
    fn test_resolve_scope_nested_derived_table() {
        let sql = "SELECT * FROM (SELECT * FROM (SELECT * FROM deep) AS mid) AS outer WHERE ";
        let tokens = tokenize(sql);
        let scope = resolve_scope(&tokens, sql);
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
