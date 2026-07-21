//! SQL tokenizer — position-aware tokenization for context analysis.
//!
//! Properly handles string/comment awareness, hyphenated identifiers,
//! dollar-quoted strings, and escaped quotes. Produces a flat token list
//! with byte positions suitable for cursor-offset lookup.

// ---------------------------------------------------------------------------
// Token types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Whitespace,
    LineComment,
    BlockComment,
    Ident,       // plain identifier
    QuotedIdent, // "double-quoted" identifier
    Keyword,
    StrLit, // 'single-quoted string'
    NumLit, // numeric literal
    Op,     // = > < >= <= != <>
    Dot,
    Comma,
    Semi,
    LParen,
    RParen,
    At,        // @ prefix (for @connection, @database)
    DollarStr, // $$dollar-quoted string$$
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Token {
    pub(crate) fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }

    pub(crate) fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }

    /// Return the display text of the token, stripping quotes for QuotedIdent.
    /// For a backtick-quoted or double-quoted identifier, returns the inner text
    /// without the quote characters.
    pub(crate) fn display_text<'a>(&self, sql: &'a str) -> &'a str {
        match self.kind {
            TokenKind::QuotedIdent if self.end - self.start >= 2 => {
                &sql[self.start + 1..self.end - 1]
            }
            _ => self.text(sql),
        }
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// Tokenize SQL text. Returns tokens with byte positions.
pub(crate) fn tokenize(sql: &str) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let n = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < n {
        let start = i;
        match bytes[i] {
            // Whitespace
            b' ' | b'\t' | b'\n' | b'\r' => {
                while i < n && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Whitespace,
                    start,
                    end: i,
                });
            }
            // Line comment
            b'-' if i + 1 < n && bytes[i + 1] == b'-' => {
                i += 2;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::LineComment,
                    start,
                    end: i,
                });
            }
            // Block comment
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 < n {
                    i += 2;
                } else {
                    i = n;
                }
                tokens.push(Token {
                    kind: TokenKind::BlockComment,
                    start,
                    end: i,
                });
            }
            // Single-quoted string
            b'\'' => {
                i += 1;
                while i < n {
                    if bytes[i] == b'\'' {
                        i += 1;
                        // Handle escaped single-quote ''
                        if i < n && bytes[i] == b'\'' {
                            i += 1; // skip second quote of ''
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::StrLit,
                    start,
                    end: i,
                });
            }
            // Double-quoted identifier
            b'"' => {
                i += 1;
                while i < n && bytes[i] != b'"' {
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::QuotedIdent,
                    start,
                    end: i,
                });
            }
            // Backtick-quoted identifier (MySQL)
            b'`' => {
                i += 1;
                while i < n && bytes[i] != b'`' {
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::QuotedIdent,
                    start,
                    end: i,
                });
            }
            // Dollar-quoted string ($$...$$)
            b'$' if i + 1 < n && bytes[i + 1] == b'$' => {
                i += 2;
                while i + 1 < n && !(bytes[i] == b'$' && bytes[i + 1] == b'$') {
                    i += 1;
                }
                if i + 1 < n {
                    i += 2;
                } else {
                    i = n;
                }
                tokens.push(Token {
                    kind: TokenKind::DollarStr,
                    start,
                    end: i,
                });
            }
            // @ directive (capture @ + following identifier as a single token)
            b'@' => {
                i += 1;
                while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::At,
                    start,
                    end: i,
                });
            }
            // Identifier or keyword (starts with letter or underscore)
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                while i < n
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
                {
                    i += 1;
                }
                let word = &sql[start..i];
                let kind = if is_known_keyword(word) {
                    TokenKind::Keyword
                } else {
                    TokenKind::Ident
                };
                tokens.push(Token {
                    kind,
                    start,
                    end: i,
                });
            }
            // Numeric literal
            b'0'..=b'9' => {
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::NumLit,
                    start,
                    end: i,
                });
            }
            b'.' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Dot,
                    start,
                    end: i,
                });
            }
            b',' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Comma,
                    start,
                    end: i,
                });
            }
            b';' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Semi,
                    start,
                    end: i,
                });
            }
            b'(' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    start,
                    end: i,
                });
            }
            b')' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    start,
                    end: i,
                });
            }
            b'*' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Op,
                    start,
                    end: i,
                });
            }
            b'=' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Op,
                    start,
                    end: i,
                });
            }
            // Comparison operators
            b'!' if i + 1 < n && bytes[i + 1] == b'=' => {
                i += 2;
                tokens.push(Token {
                    kind: TokenKind::Op,
                    start,
                    end: i,
                });
            }
            b'<' | b'>' => {
                i += 1;
                if i < n && bytes[i] == b'=' {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Op,
                    start,
                    end: i,
                });
            }
            // standalone hyphen (minus operator)
            b'-' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Op,
                    start,
                    end: i,
                });
            }
            // Any other character — skip
            _ => {
                i += 1;
            }
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// Keyword helpers
// ---------------------------------------------------------------------------

/// Case-insensitive keyword equality check.
pub(crate) fn kw_eq(actual: &str, expected: &str) -> bool {
    actual.len() == expected.len()
        && actual
            .as_bytes()
            .iter()
            .zip(expected.as_bytes())
            .all(|(a, e)| a.eq_ignore_ascii_case(e))
}

/// Check if a word is a known SQL keyword.
pub(crate) fn is_known_keyword(word: &str) -> bool {
    let w = word.as_bytes();
    let up = |b: u8| if b.is_ascii_lowercase() { b - 32 } else { b };

    if w.len() == 1 {
        return false;
    }

    let mut buf = [0u8; 24];
    if w.len() > buf.len() {
        return false;
    }
    for (i, &b) in w.iter().enumerate() {
        buf[i] = up(b);
    }
    let up_slice = &buf[..w.len()];

    const KWS: &[&[u8]] = &[
        b"ADD",
        b"AFTER",
        b"ALL",
        b"ALTER",
        b"ANALYZE",
        b"AND",
        b"ANY",
        b"AS",
        b"ASC",
        b"AUTO_INCREMENT",
        b"AUTOINCREMENT",
        b"AVG",
        b"BEGIN",
        b"BETWEEN",
        b"BY",
        b"BOOL",
        b"CALL",
        b"CASCADE",
        b"CASE",
        b"CAST",
        b"CHAR",
        b"CLUSTER",
        b"COALESCE",
        b"COLUMN",
        b"COLUMNS",
        b"COMMENT",
        b"COMMIT",
        b"COPY",
        b"COUNT",
        b"CREATE",
        b"CROSS",
        b"CURRENT_DATE",
        b"CURRENT_TIMESTAMP",
        b"DEALLOCATE",
        b"DECIMAL",
        b"DEFAULT",
        b"DELETE",
        b"DESC",
        b"DISTINCT",
        b"DO",
        b"DOUBLE",
        b"DROP",
        b"ELSE",
        b"END",
        b"EXCEPT",
        b"EXECUTE",
        b"EXISTS",
        b"EXPLAIN",
        b"FALSE",
        b"FIELDS",
        b"FLOAT",
        b"FOR",
        b"FOREIGN",
        b"FROM",
        b"FULL",
        b"GRANT",
        b"GROUP",
        b"HAVING",
        b"ILIKE",
        b"IN",
        b"INDEX",
        b"INNER",
        b"INSERT",
        b"INT",
        b"INTEGER",
        b"INTERSECT",
        b"INTO",
        b"IS",
        b"JOIN",
        b"KEY",
        b"LEFT",
        b"LIKE",
        b"LIMIT",
        b"LISTEN",
        b"LOCK",
        b"LOCKED",
        b"LOWER",
        b"MAX",
        b"MIN",
        b"MODIFY",
        b"NATURAL",
        b"NOT",
        b"NOTIFY",
        b"NOWAIT",
        b"NULL",
        b"NULLIF",
        b"NUMERIC",
        b"OF",
        b"OFFSET",
        b"ON",
        b"OR",
        b"ORDER",
        b"OUTER",
        b"OVER",
        b"PARTITION",
        b"PREPARE",
        b"PRIMARY",
        b"REAL",
        b"REINDEX",
        b"REFERENCES",
        b"RENAME",
        b"REPLACE",
        b"RETURNING",
        b"REVOKE",
        b"RIGHT",
        b"ROLLBACK",
        b"DATABASES",
        b"SCHEMAS",
        b"SELECT",
        b"SERIAL",
        b"SET",
        b"SHARE",
        b"SHOW",
        b"SKIP",
        b"SMALLINT",
        b"TABLE",
        b"TABLES",
        b"TEXT",
        b"THEN",
        b"TIME",
        b"TIMESTAMP",
        b"TINYINT",
        b"TRIM",
        b"TRUE",
        b"TRUNCATE",
        b"UNION",
        b"UNIQUE",
        b"UPDATE",
        b"USE",
        b"USING",
        b"UUID",
        b"VACUUM",
        b"VALUES",
        b"VARCHAR",
        b"WHEN",
        b"WHERE",
        b"WITH",
    ];

    KWS.contains(&up_slice)
}

pub(crate) fn is_table_keyword(w: &str) -> bool {
    matches!(
        w,
        "analyze" | "call" | "copy" | "from" | "into" | "join" | "table" | "update" | "vacuum"
    )
}

pub(crate) fn is_column_keyword(w: &str) -> bool {
    matches!(
        w,
        "where"
            | "set"
            | "on"
            | "having"
            | "select"
            | "and"
            | "or"
            | "not"
            | "by"
            | "distinct"
            | "returning"
            | "all"
            | "after"
    )
}

pub(crate) fn is_predicate_keyword(w: &str) -> bool {
    matches!(w, "in" | "between" | "like" | "ilike" | "is" | "exists")
}

// ---------------------------------------------------------------------------
// Token navigation helpers
// ---------------------------------------------------------------------------

/// Find the index of the Token that contains `offset`. Handles cursor at end of input.
pub(crate) fn find_token_at_offset(tokens: &[Token], offset: usize) -> Option<usize> {
    if tokens.is_empty() {
        return None;
    }

    let idx = tokens.partition_point(|t| t.end <= offset);

    if idx < tokens.len() && tokens[idx].start <= offset && offset < tokens[idx].end {
        return Some(idx);
    }

    if idx < tokens.len() && offset == tokens[idx].start {
        return Some(idx);
    }

    for i in (0..tokens.len()).rev() {
        match tokens[i].kind {
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => continue,
            _ => return Some(i),
        }
    }

    None
}

/// Scan backward from a token index, skipping whitespace and comments.
pub(crate) fn skip_back(tokens: &[Token], mut i: usize) -> Option<usize> {
    loop {
        if i == 0 {
            return None;
        }
        i -= 1;
        match tokens[i].kind {
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => continue,
            _ => return Some(i),
        }
    }
}

/// Scan forward from a token index, skipping whitespace and comments.
pub(crate) fn skip_forward(tokens: &[Token], mut i: usize) -> Option<usize> {
    while i + 1 < tokens.len() {
        i += 1;
        match tokens[i].kind {
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => continue,
            _ => return Some(i),
        }
    }
    None
}

/// Extract the prefix string at the cursor position from the token stream.
pub(crate) fn extract_prefix(sql: &str, offset: usize, tokens: &[Token], idx: usize) -> String {
    if idx < tokens.len() && tokens[idx].contains(offset) && tokens[idx].start < offset {
        let t = &tokens[idx];
        match t.kind {
            TokenKind::Ident | TokenKind::Keyword | TokenKind::NumLit | TokenKind::At => {
                return sql[t.start..offset].to_string();
            }
            TokenKind::QuotedIdent => {
                return sql[t.start + 1..offset].to_string();
            }
            _ => {}
        }
    }
    if idx < tokens.len() {
        match tokens[idx].kind {
            TokenKind::Ident | TokenKind::Keyword | TokenKind::NumLit | TokenKind::At => {
                return tokens[idx].text(sql).to_string();
            }
            TokenKind::QuotedIdent => {
                return tokens[idx].display_text(sql).to_string();
            }
            _ => {}
        }
    }
    if idx > 0 {
        let prev = idx - 1;
        if prev < tokens.len() {
            match tokens[prev].kind {
                TokenKind::Ident | TokenKind::Keyword | TokenKind::NumLit
                    if tokens[prev].end == offset =>
                {
                    return tokens[prev].text(sql).to_string();
                }
                TokenKind::QuotedIdent if tokens[prev].end == offset => {
                    return tokens[prev].display_text(sql).to_string();
                }
                _ => {}
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift check: every structural single-word SQL keyword from Lua KEYWORDS
    /// must be recognized by Rust's is_known_keyword() for correct token classification.
    /// Compound snippets (e.g. "ORDER BY") are display-only and not checked.
    #[test]
    fn test_lua_keywords_recognized_by_rust() {
        let single_word_keywords: &[&str] = &[
            "SELECT",
            "FROM",
            "WHERE",
            "JOIN",
            "ON",
            "HAVING",
            "LIMIT",
            "OFFSET",
            "DISTINCT",
            "ALL",
            "UNION",
            "AS",
            "WITH",
            "VALUES",
            "UPDATE",
            "SET",
            "AND",
            "OR",
            "NOT",
            "IN",
            "EXISTS",
            "IS",
            "NULL",
            "LIKE",
            "ILIKE",
            "BETWEEN",
            "UNIQUE",
            "DEFAULT",
            "REFERENCES",
            "COMMENT",
            "AFTER",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "DESC",
            "SHOW",
            "USE",
            "DELETE",
            "ADD",
            "DROP",
            "RENAME",
            "MODIFY",
            "AUTO_INCREMENT",
            "AUTOINCREMENT",
        ];
        for &kw in single_word_keywords {
            assert!(
                is_known_keyword(kw),
                "Lua keyword '{}' is not in Rust's is_known_keyword() — tokenizer classifies it as Ident",
                kw,
            );
        }
    }
}
