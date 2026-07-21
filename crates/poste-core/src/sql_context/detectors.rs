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
