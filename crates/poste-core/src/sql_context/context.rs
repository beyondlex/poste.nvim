use super::detectors::{
    try_bare_set, try_directive, try_dot_column, try_for_update_of, try_grant_revoke,
    try_insert_column, try_show_statement,
};
use super::scanner::detect_scan_backward;
use super::scope;
use super::statements;
use super::tokenizer::{extract_prefix, find_token_at_offset, tokenize, TokenKind};
use super::{functions, ContextResult, ContextType, SqlDialect};

pub fn detect_context(sql: &str, offset: usize) -> Option<ContextResult> {
    detect_context_with_dialect(sql, offset, SqlDialect::Generic)
}

pub fn detect_context_with_dialect(
    sql: &str,
    offset: usize,
    dialect: SqlDialect,
) -> Option<ContextResult> {
    let tokens = tokenize(sql);
    if tokens.is_empty() {
        return Some(ContextResult {
            context_type: ContextType::Keyword,
            tables: vec![],
            prefix: String::new(),
            functions: functions::known_functions_for_dialect(dialect),
            in_string: false,
            in_comment: false,
        });
    }

    let cursor_idx_raw = find_token_at_offset(&tokens, offset).unwrap_or(0);
    let cursor_idx = if cursor_idx_raw + 1 < tokens.len() && offset > tokens[cursor_idx_raw].end {
        cursor_idx_raw + 1
    } else {
        cursor_idx_raw
    };
    let cursor_tok = &tokens[cursor_idx];

    let in_string = cursor_tok.kind == TokenKind::StrLit;
    let in_comment = matches!(
        cursor_tok.kind,
        TokenKind::LineComment | TokenKind::BlockComment
    );
    if in_string {
        let funcs = functions::known_functions_for_dialect(dialect);
        return Some(ContextResult {
            context_type: ContextType::String,
            tables: vec![],
            prefix: String::new(),
            functions: funcs,
            in_string: true,
            in_comment: false,
        });
    }
    if in_comment {
        let funcs = functions::known_functions_for_dialect(dialect);
        return Some(ContextResult {
            context_type: ContextType::Comment,
            tables: vec![],
            prefix: String::new(),
            functions: funcs,
            in_string: false,
            in_comment: true,
        });
    }

    let prefix = extract_prefix(sql, offset, &tokens, cursor_idx);

    let (stmt_start, stmt_end) = statements::find_statement_token_range(&tokens, cursor_idx, sql);
    let stmt_tokens = &tokens[stmt_start..stmt_end];

    let scope = scope::resolve_scope(stmt_tokens, sql);
    let tables = scope.tables;
    let functions = functions::known_functions_for_dialect(dialect);

    if let Some(ctx) = try_dot_column(&tokens, cursor_idx, sql) {
        return Some(ContextResult {
            context_type: ctx,
            tables,
            prefix,
            functions,
            in_string: false,
            in_comment: false,
        });
    }

    if let Some(ctx) = try_insert_column(&tokens, cursor_idx, sql) {
        return Some(ContextResult {
            context_type: ctx,
            tables,
            prefix,
            functions,
            in_string: false,
            in_comment: false,
        });
    }

    if let Some(ctx) = try_directive(&tokens, cursor_idx, sql) {
        return Some(ContextResult {
            context_type: ctx,
            tables,
            prefix,
            functions,
            in_string: false,
            in_comment: false,
        });
    }

    if let Some(ctx) = try_show_statement(&tokens, cursor_idx, sql) {
        return Some(ContextResult {
            context_type: ctx,
            tables,
            prefix,
            functions,
            in_string: false,
            in_comment: false,
        });
    }

    if let Some(ctx) = try_grant_revoke(&tokens, cursor_idx, sql) {
        return Some(ContextResult {
            context_type: ctx,
            tables,
            prefix,
            functions,
            in_string: false,
            in_comment: false,
        });
    }

    if let Some(ctx) = try_for_update_of(&tokens, cursor_idx, sql) {
        return Some(ContextResult {
            context_type: ctx,
            tables,
            prefix,
            functions,
            in_string: false,
            in_comment: false,
        });
    }

    if let Some(ctx) = try_bare_set(&tokens, cursor_idx, sql) {
        return Some(ContextResult {
            context_type: ctx,
            tables,
            prefix,
            functions,
            in_string: false,
            in_comment: false,
        });
    }

    let cursor_on_ident = matches!(
        cursor_tok.kind,
        TokenKind::Ident
            | TokenKind::QuotedIdent
            | TokenKind::Keyword
            | TokenKind::NumLit
            | TokenKind::At
    ) || (matches!(cursor_tok.kind, TokenKind::Whitespace)
        && offset > 0
        && offset <= sql.len()
        && sql[..offset]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_'));
    let context_type = detect_scan_backward(&tokens, cursor_idx, sql, cursor_on_ident);

    Some(ContextResult {
        context_type,
        tables,
        prefix,
        functions,
        in_string: false,
        in_comment: false,
    })
}
