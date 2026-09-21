//! SQL tokenizer — position-aware tokenization for context analysis.
//!
//! Properly handles string/comment awareness, hyphenated identifiers,
//! dollar-quoted strings, and escaped quotes. Produces a flat token list
//! with byte positions suitable for cursor-offset lookup.

use crate::sql_parser::QuoteEscapes;

use super::SqlDialect;

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

/// Tokenize SQL text with the standard (`''`-only) quote reading.
pub(crate) fn tokenize(sql: &str) -> Vec<Token> {
    tokenize_with(sql, QuoteEscapes::Standard)
}

/// Tokenize SQL text, reading string literals the way `escapes` says the
/// dialect does. Returns tokens with byte positions.
pub(crate) fn tokenize_with(sql: &str, escapes: QuoteEscapes) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let n = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    let backslash = escapes == QuoteEscapes::Backslash;

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
                // `E'…'` is Postgres' explicit opt-in to C-style escapes, so
                // inside one run a backslash always escapes the next character;
                // `escapes` adds the dialect-wide form of the same rule.
                let esc = backslash || c_escapes_prefix(bytes, start);
                i = scan_quoted(bytes, i, b'\'', esc);
                tokens.push(Token {
                    kind: TokenKind::StrLit,
                    start,
                    end: i,
                });
            }
            // Double-quoted identifier
            b'"' => {
                i = scan_quoted(bytes, i, b'"', backslash);
                tokens.push(Token {
                    kind: TokenKind::QuotedIdent,
                    start,
                    end: i,
                });
            }
            // Backtick-quoted identifier (MySQL)
            b'`' => {
                i = scan_quoted(bytes, i, b'`', backslash);
                tokens.push(Token {
                    kind: TokenKind::QuotedIdent,
                    start,
                    end: i,
                });
            }
            // Dollar-quoted string ($$…$$ or $tag$…$tag$)
            b'$' => {
                if let Some(end) = scan_dollar_quote(bytes, i) {
                    i = end;
                    tokens.push(Token {
                        kind: TokenKind::DollarStr,
                        start,
                        end: i,
                    });
                } else {
                    // `$1`, `$name` without a closing `$` — a parameter
                    // placeholder or an ordinary character, not a string.
                    i += 1;
                }
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
            // Numeric literal, or a digit-leading identifier (e.g. `23_tablename`
            // — legal unquoted in MySQL/MariaDB). If digits continue directly into
            // identifier characters (letter or underscore, NOT `-` so arithmetic
            // like `10-2` keeps its operator), consume the whole word as one Ident
            // so table/column context detection sees a single token.
            b'0'..=b'9' => {
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < n && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                    while i < n
                        && (bytes[i].is_ascii_alphanumeric()
                            || bytes[i] == b'_'
                            || bytes[i] == b'-')
                    {
                        i += 1;
                    }
                    tokens.push(Token {
                        kind: TokenKind::Ident,
                        start,
                        end: i,
                    });
                } else {
                    tokens.push(Token {
                        kind: TokenKind::NumLit,
                        start,
                        end: i,
                    });
                }
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
// Quoted-run scanners
// ---------------------------------------------------------------------------

/// The quote-escape reading a dialect's literals need. This is the tokenizer's
/// view of the same rule [`QuoteEscapes::for_protocol`] gives the splitter, so
/// the two scanners cannot disagree about where a literal ends — and `Generic`
/// keeps the standard reading, because a bare `\` is only an escape in MySQL,
/// and a caller that knows it is on MySQL says so through `--dialect`.
pub(crate) fn escapes_for(dialect: SqlDialect) -> QuoteEscapes {
    match dialect {
        SqlDialect::MySql => QuoteEscapes::Backslash,
        _ => QuoteEscapes::Standard,
    }
}

/// Scan a run quoted by `quote`, starting at its opening character. A doubled
/// quote (`''`, `""`, ` `` `) is that character escaped, not a terminator, so
/// `'it''s'` and `"my""table"` are each one run. When `backslash` is set, a
/// `\` also takes the next character out of the scan (MySQL's default
/// `sql_mode`, Postgres' `E'…'`). An unterminated run ends at the end of the
/// input; returns the index just past the closing quote.
fn scan_quoted(bytes: &[u8], start: usize, quote: u8, backslash: bool) -> usize {
    let n = bytes.len();
    let mut i = start + 1;
    while i < n {
        if backslash && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            i += 1;
            if i < n && bytes[i] == quote {
                i += 1;
                continue;
            }
            return i;
        }
        i += 1;
    }
    i
}

/// True when the `'` at `start` opens an `E'…'` (or `e'…'`) literal, which is
/// Postgres for "interpret backslashes in this literal". The `E` has to stand
/// alone: `WHERE e'x'` is that, `SELECT foobar_e'x'` is an identifier followed
/// by an ordinary string.
fn c_escapes_prefix(bytes: &[u8], start: usize) -> bool {
    if !matches!(
        start.checked_sub(1).and_then(|i| bytes.get(i)),
        Some(b'e' | b'E')
    ) {
        return false;
    }
    !matches!(
        start.checked_sub(2).and_then(|i| bytes.get(i)),
        Some(&c) if c.is_ascii_alphanumeric() || c == b'_'
    )
}

/// If the `$` at `start` opens a Postgres dollar quote (`$$`, or `$tag$` with
/// `tag` matching `[A-Za-z_][A-Za-z0-9_]*`), return the index just past the
/// matching closing delimiter; a body with no closing delimiter runs to the end
/// of the input, which is how `sql_parser::split_statements` reads it too.
/// `None` means this `$` opens nothing — a `$1`-style placeholder, or a tag
/// whose closing `$` is missing — and is tokenized as an ordinary character.
///
/// The tag grammar is deliberately the splitter's and not wider: the two
/// scanners answer questions about the same text (what is one statement, where
/// does this string end), and a rule they disagree on is a bug that only shows
/// up in the editor.
fn scan_dollar_quote(bytes: &[u8], start: usize) -> Option<usize> {
    let n = bytes.len();
    let mut j = start + 1;
    if bytes.get(j) == Some(&b'$') {
        j += 1;
    } else if matches!(bytes.get(j), Some(&c) if c.is_ascii_alphabetic() || c == b'_') {
        j += 1;
        while matches!(bytes.get(j), Some(&c) if c.is_ascii_alphanumeric() || c == b'_') {
            j += 1;
        }
        if bytes.get(j) != Some(&b'$') {
            return None;
        }
        j += 1;
    } else {
        return None;
    }
    let delim = &bytes[start..j];
    let mut k = j;
    while k < n {
        let found = bytes[k..].iter().position(|&b| b == b'$')?;
        k += found;
        if bytes.len() - k >= delim.len() && &bytes[k..k + delim.len()] == delim {
            return Some(k + delim.len());
        }
        k += 1;
    }
    Some(n)
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
        b"CHARACTER",
        b"CLUSTER",
        b"COALESCE",
        b"COLLATE",
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
        b"DUPLICATE",
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
        b"GLOB",
        b"GRANT",
        b"GROUP",
        b"HAVING",
        b"IF",
        b"ILIKE",
        b"IN",
        b"INDEX",
        b"INNER",
        b"INSERT",
        b"INT",
        b"INTEGER",
        b"INTERSECT",
        b"INVISIBLE",
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
        b"PLAN",
        b"PRAGMA",
        b"PREPARE",
        b"PRIMARY",
        b"QUERY",
        b"REAL",
        b"RECURSIVE",
        b"REINDEX",
        b"RELEASE",
        b"REFERENCES",
        b"RENAME",
        b"REPLACE",
        b"RETURNING",
        b"REVOKE",
        b"RIGHT",
        b"ROLLBACK",
        b"DATABASES",
        b"SCHEMAS",
        b"SAVEPOINT",
        b"SELECT",
        b"SEQUENCE",
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

/// Keywords after which the next token names a relation to complete against:
/// query clauses (`from`, `into`, `join`, `update`), DDL/DML heads (`table`,
/// `sequence`, `copy`, `call`, `analyze`, `vacuum`) and `references`, whose
/// foreign-key target is a table like any other.
///
/// `update` also matches MySQL's `ON DUPLICATE KEY UPDATE`, where the next word
/// is a column — `scanner::detect_scan_backward` special-cases that.
pub(crate) fn is_table_keyword(w: &str) -> bool {
    matches!(
        w,
        "analyze"
            | "call"
            | "copy"
            | "from"
            | "into"
            | "join"
            | "references"
            | "sequence"
            | "table"
            | "update"
            | "vacuum"
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

/// Set operators (`w` lowercased): they join two query blocks into one
/// statement while keeping each block's `FROM` list its own scope.
///
/// `MINUS` is deliberately absent — Oracle has it as a set operator, Postgres
/// does not, and treating a `minus` identifier as one would silently mis-scope.
pub(crate) fn is_set_operator(w: &str) -> bool {
    matches!(w, "union" | "intersect" | "except")
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

/// Is the `UPDATE` at `kw_idx` the assignment clause of MySQL's upsert
/// (`INSERT … ON DUPLICATE KEY UPDATE col = …`)?
///
/// There the next word is a column of the insert target, not a relation name —
/// without this, `is_table_keyword("update")` makes the scanner offer tables.
pub(crate) fn is_upsert_update(tokens: &[Token], sql: &str, kw_idx: usize) -> bool {
    let mut i = kw_idx;
    for expected in ["key", "duplicate", "on"] {
        i = match skip_back(tokens, i) {
            Some(x) => x,
            None => return false,
        };
        if tokens[i].kind != TokenKind::Keyword || !kw_eq(tokens[i].text(sql), expected) {
            return false;
        }
    }
    true
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

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).into_iter().map(|t| t.kind).collect()
    }

    fn quoted_spans_with(src: &str, escapes: QuoteEscapes) -> Vec<(TokenKind, String)> {
        tokenize_with(src, escapes)
            .into_iter()
            .filter(|t| {
                matches!(
                    t.kind,
                    TokenKind::StrLit | TokenKind::DollarStr | TokenKind::QuotedIdent
                )
            })
            .map(|t| (t.kind.clone(), t.text(src).to_string()))
            .collect()
    }

    fn quoted_spans(src: &str) -> Vec<(TokenKind, String)> {
        quoted_spans_with(src, QuoteEscapes::Standard)
    }

    /// `$tag$ … $tag$` is the usual spelling of a function or DO-block body, and
    /// the splitter (`sql_parser::split_statements`) has read tags for a while.
    /// Reading only `$$` here meant a body's quotes and semicolons leaked into
    /// the token stream: everything after an odd `'` inside the body became one
    /// string, so completion, the table list and `context stmt`'s line range all
    /// ran off the end of the block.
    #[test]
    fn tagged_dollar_quote_is_one_token() {
        let src = "SELECT $fn$ BEGIN 'x; END $fn$ AS body FROM t";
        assert_eq!(
            quoted_spans(src),
            vec![(TokenKind::DollarStr, "$fn$ BEGIN 'x; END $fn$".to_string())],
            "the body must be one token, its `'` and `;` are content"
        );
    }

    /// A bare `$` is not a tag: `$1` is a parameter placeholder and `$` alone
    /// ends the input. Both must stay out of string mode, or the *whole* rest of
    /// the buffer is treated as a literal and completion stops working.
    #[test]
    fn non_tags_stay_out_of_dollar_string() {
        assert_eq!(
            quoted_spans("SELECT $1, $fn$ a $fn$ FROM t"),
            vec![(TokenKind::DollarStr, "$fn$ a $fn$".to_string())]
        );
        assert!(
            !kinds("SELECT $ FROM t").contains(&TokenKind::DollarStr),
            "a lone `$` is not an opener"
        );
        assert!(
            !kinds("SELECT $a FROM t").contains(&TokenKind::DollarStr),
            "an unterminated tag is not an opener"
        );
    }

    /// `""` and ``` `` ``` are how a quote character is written inside a quoted
    /// identifier (`"my""table"` names the table `my"table`), exactly like `''`
    /// inside a string — which this tokenizer already handled. Stopping at the
    /// first inner quote split one identifier into two tokens and re-opened at
    /// the next, so the name the completion lookup asks for was wrong and the
    /// tokens after it were shifted by one quote.
    #[test]
    fn doubled_quotes_stay_inside_the_identifier() {
        assert_eq!(
            quoted_spans("SELECT * FROM \"my\"\"table\" WHERE a = 1"),
            vec![(TokenKind::QuotedIdent, "\"my\"\"table\"".to_string())]
        );
        assert_eq!(
            quoted_spans("SELECT * FROM `a``b` WHERE a = 1"),
            vec![(TokenKind::QuotedIdent, "`a``b`".to_string())]
        );
        assert_eq!(
            quoted_spans("SELECT 'it''s' AS x"),
            vec![(TokenKind::StrLit, "'it''s'".to_string())]
        );
    }

    /// MySQL's default `sql_mode` reads `\'` as an escaped quote, which is the
    /// form `mysqldump` writes. Under the standard reading the literal ends
    /// early, the next `'` opens another, and everything after it — including
    /// the `SELECT` the user is typing into — arrives as one string, so
    /// completion reports "inside a string" and offers nothing for the rest of
    /// the block. This is the tokenizer's side of the rule the splitter already
    /// applies per dialect.
    #[test]
    fn mysql_reads_backslash_escapes() {
        let src = "INSERT INTO t VALUES ('it\\'s here'); SELECT * FROM users WHERE a = 1";
        // The standard reading is the bug: the literal closes at `it\`, the next
        // `'` re-opens one that has no partner, and the `SELECT` after it is
        // inside a string as far as the tokenizer is concerned.
        assert!(
            !tokenize_with(src, QuoteEscapes::Standard)
                .iter()
                .any(|t| t.kind == TokenKind::Keyword && t.text(src) == "SELECT"),
            "expected the standard reading to lose the SELECT — that is the bug"
        );
        let mysql = tokenize_with(src, QuoteEscapes::Backslash);
        assert_eq!(
            quoted_spans_with(src, QuoteEscapes::Backslash),
            vec![(TokenKind::StrLit, "'it\\'s here'".to_string())]
        );
        assert!(
            mysql
                .iter()
                .any(|t| t.kind == TokenKind::Keyword && t.text(src) == "SELECT"),
            "the statement after a dumped row must still be there"
        );
    }

    /// `E'…'` opts into backslash escapes for that one literal in Postgres,
    /// whatever the dialect default is — the prefix is the caller saying so.
    /// Under the plain standard reading `E'\''` ends at the wrong quote and
    /// takes the rest of the buffer with it.
    #[test]
    fn e_prefixed_literal_reads_backslashes_too() {
        let src = "SELECT E'\\'' AS x, name FROM t";
        let toks = tokenize_with(src, QuoteEscapes::Standard);
        assert_eq!(
            toks.iter()
                .filter(|t| t.kind == TokenKind::StrLit)
                .map(|t| t.text(src))
                .collect::<Vec<_>>(),
            vec!["'\\''"],
            "the literal is `'\\''` — one escaped quote, then the close"
        );
        assert!(toks
            .iter()
            .any(|t| t.kind == TokenKind::Keyword && t.text(src) == "FROM"));
        // An `e` that is the tail of an identifier is not the prefix.
        assert_eq!(
            quoted_spans("SELECT name'x' AS y"),
            vec![(TokenKind::StrLit, "'x'".to_string())],
            "name'x' is an identifier followed by an ordinary literal"
        );
    }
}
