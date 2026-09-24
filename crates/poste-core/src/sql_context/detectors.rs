use super::tokenizer::{is_table_keyword, kw_eq, skip_back, skip_forward, Token, TokenKind};
use super::ContextType;

pub(crate) fn try_dot_column(
    tokens: &[Token],
    cursor_idx: usize,
    sql: &str,
) -> Option<ContextType> {
    let check_dot = |dot_idx: usize| -> Option<ContextType> {
        if dot_idx == 0 {
            return None;
        }
        let prev = dot_idx - 1;
        let prev_idx = match tokens[prev].kind {
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => {
                skip_back(tokens, dot_idx)?
            }
            _ => prev,
        };

        let prev_tok = &tokens[prev_idx];
        match prev_tok.kind {
            TokenKind::Ident | TokenKind::QuotedIdent | TokenKind::Keyword => {
                let ident = prev_tok.display_text(sql).to_string();
                if let Some(ctx_kw_idx) = skip_back(tokens, prev_idx) {
                    if tokens[ctx_kw_idx].kind == TokenKind::Keyword {
                        let kw = tokens[ctx_kw_idx].text(sql).to_ascii_lowercase();
                        if is_table_keyword(&kw) {
                            return Some(ContextType::SchemaTable { schema: ident });
                        }
                    }
                }
                let mut schema = None;
                if let Some(before) = skip_back(tokens, prev_idx) {
                    if tokens[before].kind == TokenKind::Dot {
                        if let Some(schema_tok_idx) = skip_back(tokens, before) {
                            let schema_tok = &tokens[schema_tok_idx];
                            if matches!(schema_tok.kind, TokenKind::Ident | TokenKind::QuotedIdent)
                            {
                                schema = Some(schema_tok.display_text(sql).to_string());
                            }
                        }
                    }
                }
                Some(ContextType::DotColumn {
                    table: ident,
                    schema,
                })
            }
            _ => None,
        }
    };

    if tokens[cursor_idx].kind == TokenKind::Dot {
        return check_dot(cursor_idx);
    }

    if let Some(prev) = skip_back(tokens, cursor_idx) {
        if tokens[prev].kind == TokenKind::Dot {
            return check_dot(prev);
        }
    }

    None
}

pub(crate) fn try_insert_column(
    tokens: &[Token],
    cursor_idx: usize,
    sql: &str,
) -> Option<ContextType> {
    let mut i = cursor_idx;

    if tokens[i].kind == TokenKind::RParen {
        if let Some(prev) = skip_back(tokens, i) {
            if tokens[prev].kind == TokenKind::LParen {
                i = prev;
            } else {
                let mut found_lparen = false;
                let mut j = i;
                while let Some(idx) = skip_back(tokens, j) {
                    j = idx;
                    match tokens[idx].kind {
                        TokenKind::LParen => {
                            i = idx;
                            found_lparen = true;
                            break;
                        }
                        TokenKind::RParen | TokenKind::Semi => break,
                        _ => continue,
                    }
                }
                if !found_lparen {
                    return None;
                }
            }
        } else {
            return None;
        }
    } else if tokens[i].kind != TokenKind::LParen {
        if let Some(prev) = skip_back(tokens, i) {
            if tokens[prev].kind == TokenKind::LParen {
                i = prev;
            } else {
                let mut found_lparen = false;
                let mut j = i;
                while let Some(idx) = skip_back(tokens, j) {
                    j = idx;
                    match tokens[idx].kind {
                        TokenKind::LParen => {
                            found_lparen = true;
                            i = idx;
                            break;
                        }
                        TokenKind::RParen | TokenKind::Semi => break,
                        _ => continue,
                    }
                }
                if !found_lparen {
                    return None;
                }
            }
        } else {
            return None;
        }
    }

    if let Some(tbl_idx) = skip_back(tokens, i) {
        if tokens[tbl_idx].kind == TokenKind::Semi {
            return None;
        }
        let tbl_tok = &tokens[tbl_idx];
        if !matches!(
            tbl_tok.kind,
            TokenKind::Ident | TokenKind::QuotedIdent | TokenKind::Keyword
        ) {
            return None;
        }
        let table = tbl_tok.display_text(sql).to_string();

        if let Some(into_idx) = skip_back(tokens, tbl_idx) {
            if tokens[into_idx].kind == TokenKind::Semi {
                return None;
            }
            let into_tok = &tokens[into_idx];
            if into_tok.kind == TokenKind::Keyword && kw_eq(into_tok.text(sql), "into") {
                if let Some(insert_idx) = skip_back(tokens, into_idx) {
                    if tokens[insert_idx].kind == TokenKind::Semi {
                        return None;
                    }
                    let insert_tok = &tokens[insert_idx];
                    if insert_tok.kind == TokenKind::Keyword
                        && kw_eq(insert_tok.text(sql), "insert")
                    {
                        return Some(ContextType::InsertColumn { table });
                    }
                }
            }
        }

        if let Some(prev_idx) = skip_back(tokens, tbl_idx) {
            if tokens[prev_idx].kind == TokenKind::Semi {
                return None;
            }
            let prev_tok = &tokens[prev_idx];
            if prev_tok.kind == TokenKind::Keyword && kw_eq(prev_tok.text(sql), "copy") {
                return Some(ContextType::InsertColumn { table });
            }
        }
    }

    None
}

pub(crate) fn try_directive(tokens: &[Token], cursor_idx: usize, sql: &str) -> Option<ContextType> {
    // @connection/@database: safety net only — return None (Lua handles directives).
    // Kept as a no-op to avoid unused warnings; simply doesn't match.
    if tokens[cursor_idx].kind == TokenKind::At {
        let text = tokens[cursor_idx].text(sql);
        if kw_eq(text, "@connection") || kw_eq(text, "@database") {
            return None;
        }
    }

    if let Some(prev) = skip_back(tokens, cursor_idx) {
        if tokens[prev].kind == TokenKind::At {
            let text = tokens[prev].text(sql);
            if kw_eq(text, "@connection") || kw_eq(text, "@database") {
                return None;
            }
        }
    }

    if tokens[cursor_idx].kind == TokenKind::Keyword && kw_eq(tokens[cursor_idx].text(sql), "use") {
        if let Some(next) = skip_forward(tokens, cursor_idx) {
            if matches!(tokens[next].kind, TokenKind::Ident | TokenKind::QuotedIdent) {
                return Some(ContextType::Database);
            }
        } else {
            return Some(ContextType::Database);
        }
    }

    if let Some(prev) = skip_back(tokens, cursor_idx) {
        if tokens[prev].kind == TokenKind::Semi {
            return None;
        }
        if tokens[prev].kind == TokenKind::Keyword && kw_eq(tokens[prev].text(sql), "use") {
            return Some(ContextType::Database);
        }
    }

    None
}

pub(crate) fn try_show_statement(
    tokens: &[Token],
    cursor_idx: usize,
    sql: &str,
) -> Option<ContextType> {
    let start = if matches!(
        tokens[cursor_idx].kind,
        TokenKind::Ident | TokenKind::Keyword
    ) {
        skip_back(tokens, cursor_idx)?
    } else {
        cursor_idx
    };

    let mut search = start;
    loop {
        if tokens[search].kind == TokenKind::Semi {
            return None;
        }
        if tokens[search].kind == TokenKind::Keyword && kw_eq(tokens[search].text(sql), "show") {
            break;
        }
        search = skip_back(tokens, search)?;
    }

    let mut next = search;
    let mut show_type: Option<String> = None;

    while let Some(idx) = skip_forward(tokens, next) {
        if idx > cursor_idx {
            break;
        }
        match tokens[idx].kind {
            TokenKind::Keyword | TokenKind::Ident => {
                let text = tokens[idx].text(sql);
                if let Some(ty) = show_type_keyword(text) {
                    show_type = Some(ty.to_string());
                }
            }
            _ => {}
        }
        next = idx;
    }

    match show_type.as_deref() {
        Some("databases") | Some("schemas") => Some(ContextType::Database),
        Some("tables") => Some(ContextType::Table),
        Some("columns") | Some("fields") => Some(ContextType::Table),
        _ => None,
    }
}

pub(crate) fn try_grant_revoke(
    tokens: &[Token],
    cursor_idx: usize,
    sql: &str,
) -> Option<ContextType> {
    let start = if matches!(
        tokens[cursor_idx].kind,
        TokenKind::Ident | TokenKind::Keyword
    ) {
        skip_back(tokens, cursor_idx)?
    } else {
        cursor_idx
    };

    let mut search = start;
    loop {
        if tokens[search].kind == TokenKind::Semi {
            return None;
        }
        if tokens[search].kind == TokenKind::Keyword && kw_eq(tokens[search].text(sql), "on") {
            break;
        }
        if search == 0 {
            return None;
        }
        search = skip_back(tokens, search)?;
    }

    let mut before_on = search;
    loop {
        match skip_back(tokens, before_on) {
            Some(idx) => {
                if tokens[idx].kind == TokenKind::Semi {
                    return None;
                }
                let tok = &tokens[idx];
                match tok.kind {
                    TokenKind::Keyword => {
                        let kw = tok.text(sql).to_ascii_lowercase();
                        if kw == "grant" || kw == "revoke" {
                            return Some(ContextType::Table);
                        }
                        if kw == "on" {
                            return None;
                        }
                        before_on = idx;
                    }
                    TokenKind::Ident | TokenKind::Comma => {
                        before_on = idx;
                    }
                    _ => return None,
                }
            }
            None => return None,
        }
    }
}

pub(crate) fn try_for_update_of(
    tokens: &[Token],
    cursor_idx: usize,
    sql: &str,
) -> Option<ContextType> {
    let start = if matches!(
        tokens[cursor_idx].kind,
        TokenKind::Ident | TokenKind::Keyword
    ) {
        skip_back(tokens, cursor_idx)?
    } else {
        cursor_idx
    };

    let mut search = start;
    loop {
        if tokens[search].kind == TokenKind::Semi {
            return None;
        }
        if tokens[search].kind == TokenKind::Keyword && kw_eq(tokens[search].text(sql), "of") {
            break;
        }
        if search == 0 {
            return None;
        }
        search = skip_back(tokens, search)?;
    }

    let update_or_share = skip_back(tokens, search)?;
    if tokens[update_or_share].kind != TokenKind::Keyword {
        return None;
    }
    let kw = tokens[update_or_share].text(sql).to_ascii_lowercase();
    if kw != "update" && kw != "share" {
        return None;
    }

    let for_idx = skip_back(tokens, update_or_share)?;
    if tokens[for_idx].kind == TokenKind::Keyword && kw_eq(tokens[for_idx].text(sql), "for") {
        return Some(ContextType::Table);
    }

    None
}

pub(crate) fn try_bare_set(tokens: &[Token], cursor_idx: usize, sql: &str) -> Option<ContextType> {
    let start = if matches!(
        tokens[cursor_idx].kind,
        TokenKind::Ident | TokenKind::Keyword
    ) {
        skip_back(tokens, cursor_idx)?
    } else {
        cursor_idx
    };

    let mut search = start;
    loop {
        if tokens[search].kind == TokenKind::Semi {
            return None;
        }
        if tokens[search].kind == TokenKind::Keyword && kw_eq(tokens[search].text(sql), "set") {
            break;
        }
        if search == 0 {
            return None;
        }
        search = skip_back(tokens, search)?;
    }

    if let Some(prev) = skip_back(tokens, search) {
        match tokens[prev].kind {
            TokenKind::Keyword => {
                let kw = tokens[prev].text(sql).to_ascii_lowercase();
                if is_table_keyword(&kw) || kw == "lock" {
                    return None;
                }
                return Some(ContextType::Keyword);
            }
            TokenKind::Ident => {
                if let Some(before) = skip_back(tokens, prev) {
                    if tokens[before].kind == TokenKind::Keyword {
                        let kw = tokens[before].text(sql).to_ascii_lowercase();
                        if is_table_keyword(&kw) || kw == "lock" {
                            return None;
                        }
                    }
                }
                return Some(ContextType::Keyword);
            }
            _ => return Some(ContextType::Keyword),
        }
    }

    Some(ContextType::Keyword)
}

pub(crate) fn show_type_keyword(w: &str) -> Option<&'static str> {
    let w = w.as_bytes();
    if w.len() < 4 || w.len() > 9 {
        return None;
    }
    let up = |b: u8| if b.is_ascii_lowercase() { b - 32 } else { b };
    let mut buf = [0u8; 9];
    for (i, &b) in w.iter().enumerate() {
        buf[i] = up(b);
    }
    match &buf[..w.len()] {
        b"TABLES" => Some("tables"),
        b"DATABASES" => Some("databases"),
        b"SCHEMAS" => Some("schemas"),
        b"COLUMNS" => Some("columns"),
        b"FIELDS" => Some("fields"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_context::tokenizer::{tokenize, Token};

    /// Token index under the `▮` marker (editor semantics: the cursor byte
    /// belongs to the last token that starts at or before it). The marker is
    /// removed before tokenizing.
    fn marked(sql: &str) -> (Vec<Token>, usize, String) {
        let clean = sql.replace('▮', "");
        let byte = sql.find('▮').expect("sql must contain the ▮ cursor marker");
        let tokens = tokenize(&clean);
        let cursor = tokens
            .iter()
            .rposition(|t| t.start <= byte)
            .expect("cursor byte inside the token range");
        (tokens, cursor, clean)
    }

    // ---- show_type_keyword ----

    #[test]
    fn show_type_keyword_matches_any_case_exactly() {
        assert_eq!(show_type_keyword("tables"), Some("tables"));
        assert_eq!(show_type_keyword("TaBLeS"), Some("tables"));
        assert_eq!(show_type_keyword("DATABASES"), Some("databases"));
        assert_eq!(show_type_keyword("schemas"), Some("schemas"));
        assert_eq!(show_type_keyword("columns"), Some("columns"));
        assert_eq!(show_type_keyword("fields"), Some("fields"));
    }

    #[test]
    fn show_type_keyword_rejects_near_misses_and_lengths() {
        // "table" is shorter than 4..=9 bounds would allow but not a member;
        // the bounds check (4..=9 bytes) gates the long words first.
        assert_eq!(show_type_keyword("table"), None);
        assert_eq!(show_type_keyword("tabless"), None);
        assert_eq!(show_type_keyword(""), None);
        assert_eq!(show_type_keyword("database"), None);
        assert_eq!(show_type_keyword("information_schema"), None);
    }

    // ---- try_dot_column ----

    #[test]
    fn dot_after_a_table_suggests_its_columns() {
        let (tokens, cursor, sql) = marked("SELECT * FROM users WHERE users▮.");
        assert_eq!(
            try_dot_column(&tokens, cursor, &sql),
            Some(ContextType::DotColumn {
                table: "users".into(),
                schema: None,
            })
        );
    }

    #[test]
    fn dot_in_a_from_clause_names_the_schema() {
        // `FROM db.▮` is about to be a table inside schema `db`
        let (tokens, cursor, sql) = marked("SELECT * FROM db.▮");
        assert_eq!(
            try_dot_column(&tokens, cursor, &sql),
            Some(ContextType::SchemaTable {
                schema: "db".into()
            })
        );
    }

    #[test]
    fn dot_after_a_bare_word_outside_a_from_is_a_plain_column_prefix() {
        let (tokens, cursor, sql) = marked("SELECT * FROM users WHERE db.▮");
        assert_eq!(
            try_dot_column(&tokens, cursor, &sql),
            Some(ContextType::DotColumn {
                table: "db".into(),
                schema: None,
            })
        );
    }

    #[test]
    fn schema_qualified_dot_carries_both_parts() {
        let (tokens, cursor, sql) = marked("SELECT * FROM users WHERE db.users.▮");
        assert_eq!(
            try_dot_column(&tokens, cursor, &sql),
            Some(ContextType::DotColumn {
                table: "users".into(),
                schema: Some("db".into()),
            })
        );
    }

    #[test]
    fn an_ident_not_adjacent_to_a_dot_is_not_a_dot_column() {
        let (tokens, cursor, sql) = marked("SELECT * FROM publi▮c.users");
        assert_eq!(try_dot_column(&tokens, cursor, &sql), None);
    }

    // ---- try_insert_column ----

    #[test]
    fn lparen_of_insert_into_suggests_columns() {
        let (tokens, cursor, sql) = marked("INSERT INTO users (▮)");
        assert_eq!(
            try_insert_column(&tokens, cursor, &sql),
            Some(ContextType::InsertColumn {
                table: "users".into(),
            })
        );
    }

    #[test]
    fn anywhere_in_the_insert_column_list_still_suggests_columns() {
        let (tokens, cursor, sql) = marked("INSERT INTO users (id, na▮me)");
        assert!(matches!(
            try_insert_column(&tokens, cursor, &sql),
            Some(ContextType::InsertColumn { .. })
        ));
        let (tokens, cursor, sql) = marked("INSERT INTO users (id)▮");
        assert!(matches!(
            try_insert_column(&tokens, cursor, &sql),
            Some(ContextType::InsertColumn { .. })
        ));
    }

    #[test]
    fn copy_uses_the_same_column_context() {
        let (tokens, cursor, sql) = marked("COPY users (▮)");
        assert!(matches!(
            try_insert_column(&tokens, cursor, &sql),
            Some(ContextType::InsertColumn { .. })
        ));
    }

    #[test]
    fn update_and_values_parens_are_not_insert_columns() {
        let (tokens, cursor, sql) = marked("UPDATE users SET na▮me = 1");
        assert_eq!(try_insert_column(&tokens, cursor, &sql), None);
        let (tokens, cursor, sql) = marked("INSERT INTO users (id) VALUES (▮)");
        assert_eq!(try_insert_column(&tokens, cursor, &sql), None);
    }

    // ---- try_directive ----

    #[test]
    fn use_at_the_start_of_the_text_suggests_a_database() {
        let (tokens, cursor, sql) = marked("USE▮");
        assert_eq!(
            try_directive(&tokens, cursor, &sql),
            Some(ContextType::Database)
        );
        let (tokens, cursor, sql) = marked("USE my▮db");
        assert_eq!(
            try_directive(&tokens, cursor, &sql),
            Some(ContextType::Database)
        );
    }

    #[test]
    fn use_after_a_semicolon_suggests_a_database_but_the_next_statement_does_not() {
        let (tokens, cursor, sql) = marked("SELECT 1; USE▮");
        assert_eq!(
            try_directive(&tokens, cursor, &sql),
            Some(ContextType::Database)
        );
        let (tokens, cursor, sql) = marked("USE mydb; SEL▮ECT 1");
        assert_eq!(try_directive(&tokens, cursor, &sql), None);
    }

    #[test]
    fn use_fused_into_a_longer_word_is_not_a_use() {
        // `USER u` — the token under the cursor is `u`; the scanner must not
        // read `USER` as `USE`.
        let (tokens, cursor, sql) = marked("USER u▮");
        assert_eq!(try_directive(&tokens, cursor, &sql), None);
    }

    // ---- try_show_statement ----

    #[test]
    fn show_tables_and_friends_map_to_their_context() {
        let (tokens, cursor, sql) = marked("SHOW TABLE▮S");
        assert_eq!(
            try_show_statement(&tokens, cursor, &sql),
            Some(ContextType::Table)
        );
        let (tokens, cursor, sql) = marked("SHOW DA▮TABASES");
        assert_eq!(
            try_show_statement(&tokens, cursor, &sql),
            Some(ContextType::Database)
        );
        let (tokens, cursor, sql) = marked("SHOW COLUMNS FR▮OM users");
        assert_eq!(
            try_show_statement(&tokens, cursor, &sql),
            Some(ContextType::Table)
        );
    }

    #[test]
    fn show_with_a_non_type_word_and_a_plain_select_give_nothing() {
        // "CREATE" is a keyword but not a show-type — no context
        let (tokens, cursor, sql) = marked("SHOW CREATE TA▮BLE t");
        assert_eq!(try_show_statement(&tokens, cursor, &sql), None);
        let (tokens, cursor, sql) = marked("SELECT▮ 1");
        assert_eq!(try_show_statement(&tokens, cursor, &sql), None);
    }

    // ---- try_grant_revoke ----

    #[test]
    fn the_table_position_of_grant_and_revoke() {
        let (tokens, cursor, sql) = marked("GRANT SELECT, INSERT ON ta▮ble");
        assert_eq!(
            try_grant_revoke(&tokens, cursor, &sql),
            Some(ContextType::Table)
        );
        let (tokens, cursor, sql) = marked("REVOKE ALL ON s▮chema.t");
        assert_eq!(
            try_grant_revoke(&tokens, cursor, &sql),
            Some(ContextType::Table)
        );
    }

    #[test]
    fn grant_words_before_the_on_do_not_suggest_a_table() {
        let (tokens, cursor, sql) = marked("GRANT SELEC▮T, INSERT ON table");
        assert_eq!(try_grant_revoke(&tokens, cursor, &sql), None);
        let (tokens, cursor, sql) = marked("SELECT * FROM t▮");
        assert_eq!(try_grant_revoke(&tokens, cursor, &sql), None);
    }

    // ---- try_for_update_of ----

    #[test]
    fn the_table_list_after_for_update_of() {
        let (tokens, cursor, sql) = marked("SELECT * FROM t FOR UPDATE OF ta▮ble");
        assert_eq!(
            try_for_update_of(&tokens, cursor, &sql),
            Some(ContextType::Table)
        );
    }

    #[test]
    fn for_update_without_of_and_plain_ordering_are_not_table_contexts() {
        let (tokens, cursor, sql) = marked("SELECT * FROM t FOR UPDA▮TE");
        assert_eq!(try_for_update_of(&tokens, cursor, &sql), None);
        // an identifier spelled "of" outside FOR UPDATE OF must not match
        let (tokens, cursor, sql) = marked("SELECT * FROM t ORDER BY o▮f");
        assert_eq!(try_for_update_of(&tokens, cursor, &sql), None);
    }

    // ---- try_bare_set ----

    #[test]
    fn a_session_set_suggests_keywords() {
        let (tokens, cursor, sql) = marked("SET NAMES 'utf8'▮");
        assert_eq!(
            try_bare_set(&tokens, cursor, &sql),
            Some(ContextType::Keyword)
        );
        let (tokens, cursor, sql) = marked("SET SESSION sort_buffer_size = 1000▮00");
        assert_eq!(
            try_bare_set(&tokens, cursor, &sql),
            Some(ContextType::Keyword)
        );
    }

    #[test]
    fn the_set_of_update_belongs_to_the_update_not_the_session() {
        let (tokens, cursor, sql) = marked("UPDATE users SET na▮me = 1");
        assert_eq!(try_bare_set(&tokens, cursor, &sql), None);
        let (tokens, cursor, sql) = marked("LOCK TABLES t READ▮");
        assert_eq!(try_bare_set(&tokens, cursor, &sql), None);
    }
}
