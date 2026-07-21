use super::tokenizer::{
    is_column_keyword, is_predicate_keyword, is_table_keyword, kw_eq, skip_back, Token, TokenKind,
};
use super::ContextType;

pub(crate) fn detect_scan_backward(
    tokens: &[Token],
    cursor_idx: usize,
    sql: &str,
    cursor_on_ident: bool,
) -> ContextType {
    let mut i = cursor_idx;
    let mut after_comma = false;
    let mut skip_one_ident = true;

    loop {
        let tok = &tokens[i];

        match tok.kind {
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => {}
            TokenKind::Comma => {
                after_comma = true;
            }
            TokenKind::Keyword => {
                let kw = tok.text(sql).to_ascii_lowercase();
                if is_table_keyword(&kw) {
                    if !skip_one_ident && !cursor_on_ident {
                        // We already passed a table name after the keyword
                        // and the cursor is not on an identifier — the user
                        // finished the table reference, so suggest keywords
                        // (WHERE, GROUP BY, ORDER BY, etc.) instead of tables.
                        return ContextType::Keyword;
                    }
                    return ContextType::Table;
                }
                if is_column_keyword(&kw) {
                    if !skip_one_ident
                        && (kw == "select"
                            || kw == "where"
                            || kw == "having"
                            || kw == "returning"
                            || kw == "after")
                    {
                        if cursor_on_ident {
                            return ContextType::Column;
                        }
                        return ContextType::Keyword;
                    }
                    if kw == "not" {
                        if skip_one_ident {
                            if let Some(prev_idx) = skip_back(tokens, i) {
                                if tokens[prev_idx].kind == TokenKind::Ident {
                                    return ContextType::Keyword;
                                }
                            }
                            return ContextType::Column;
                        }
                        return ContextType::Keyword;
                    }
                    if kw == "all" && skip_one_ident {
                        if let Some(prev_idx) = skip_back(tokens, i) {
                            if tokens[prev_idx].kind == TokenKind::Keyword {
                                let prev_kw = tokens[prev_idx].text(sql).to_ascii_lowercase();
                                if prev_kw == "select" {
                                    return ContextType::Column;
                                }
                            }
                        }
                        return ContextType::Keyword;
                    }
                    if kw == "set" && !skip_one_ident {
                        if cursor_on_ident {
                            return ContextType::Keyword;
                        }
                        for check in (0..cursor_idx).rev() {
                            match tokens[check].kind {
                                TokenKind::Whitespace
                                | TokenKind::LineComment
                                | TokenKind::BlockComment => continue,
                                TokenKind::Comma | TokenKind::Op => {
                                    return ContextType::Column;
                                }
                                TokenKind::StrLit | TokenKind::NumLit | TokenKind::DollarStr => {
                                    return ContextType::Keyword;
                                }
                                _ => break,
                            }
                        }
                        return ContextType::Keyword;
                    }
                    return ContextType::Column;
                }
                if kw == "column" && !skip_one_ident {
                    if let Some(prev) = skip_back(tokens, i) {
                        if tokens[prev].kind == TokenKind::Keyword {
                            let prev_kw = tokens[prev].text(sql).to_ascii_lowercase();
                            if prev_kw == "add" || prev_kw == "modify" {
                                return ContextType::DataType;
                            }
                        }
                    }
                }
                if kw == "column" && skip_one_ident {
                    if let Some(prev) = skip_back(tokens, i) {
                        if tokens[prev].kind == TokenKind::Keyword
                            && kw_eq(tokens[prev].text(sql), "modify")
                        {
                            return ContextType::Column;
                        }
                    }
                }
                if after_comma {
                    after_comma = false;
                } else {
                    return ContextType::Keyword;
                }
            }
            TokenKind::Op => {
                if after_comma {
                    after_comma = false;
                    if let Some(prev) = skip_back(tokens, i) {
                        if tokens[prev].kind == TokenKind::Ident {
                            skip_one_ident = true;
                        }
                    }
                } else if let Some(prev) = skip_back(tokens, i) {
                    if tokens[prev].kind == TokenKind::Keyword {
                        let kw = tokens[prev].text(sql).to_ascii_lowercase();
                        if is_column_keyword(&kw) || is_predicate_keyword(&kw) {
                            if kw == "select" {
                                return ContextType::Keyword;
                            }
                            return ContextType::Column;
                        }
                    }
                    if tokens[prev].kind == TokenKind::Ident {
                        skip_one_ident = true;
                    } else {
                        return ContextType::Keyword;
                    }
                } else {
                    return ContextType::Keyword;
                }
            }
            TokenKind::LParen => {
                if let Some(prev) = skip_back(tokens, i) {
                    let prev_text = tokens[prev].text(sql).to_ascii_lowercase();
                    if prev_text == "in" || prev_text == "exists" {
                        return ContextType::Keyword;
                    }
                    if is_table_keyword(&prev_text) {
                        return ContextType::Keyword;
                    }
                    return ContextType::Column;
                }
                return ContextType::Keyword;
            }
            TokenKind::Ident | TokenKind::QuotedIdent | TokenKind::NumLit => {
                if after_comma {
                    after_comma = false;
                } else if skip_one_ident {
                    skip_one_ident = false;
                } else {
                    return ContextType::Keyword;
                }
            }
            TokenKind::Semi => {}
            TokenKind::RParen | TokenKind::StrLit | TokenKind::DollarStr => {}
            _ => {
                if after_comma {
                    after_comma = false;
                } else {
                    return ContextType::Keyword;
                }
            }
        }

        if i == 0 {
            break;
        }
        i -= 1;
    }

    ContextType::Keyword
}
