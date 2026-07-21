use crate::request::{Protocol, Request};
use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

pub mod vars;
pub use vars::VarResolver;

/// Structured metadata for a single `###` request block.
/// Emitted by `poste run --describe` so Lua does not re-parse HTTP semantics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockMeta {
    /// Request name from the `### Name` line (empty string if unnamed).
    pub name: String,
    /// 1-indexed start line of the block (`###` line, or 1 if no separator).
    pub line: usize,
    /// 1-indexed inclusive end line of the block content.
    pub end_line: usize,
    /// HTTP method / Redis command / SCRIPT / first token of request line.
    pub method: String,
    /// URL path (HTTP) or remainder of the request line (other protocols).
    pub path: String,
    /// Request headers as `[name, value]` pairs (HTTP only; empty otherwise).
    pub headers: Vec<(String, String)>,
    /// Request body after headers (may be empty).
    pub body: String,
    /// Full first request line (e.g. `GET https://example.com/users`).
    pub request_line: String,
}

/// A raw `###` block with absolute line numbers (1-indexed).
struct RawBlock {
    start_line: usize,
    end_line: usize,
    content: String,
}

pub struct Parser {
    env: HashMap<String, String>,
}

impl Parser {
    pub fn new(env_vars: HashMap<String, String>) -> Self {
        Self { env: env_vars }
    }

    /// Detect protocol from file extension (without the leading dot).
    pub fn detect_protocol(file_ext: &str) -> Protocol {
        match file_ext.to_lowercase().as_str() {
            "redis" => Protocol::Redis,
            "sql" => Protocol::Postgres, // default .sql to Postgres; can be overridden via @protocol directive
            "sqlite" => Protocol::Sqlite,
            "mysql" => Protocol::Mysql,
            _ => Protocol::Http, // .http, .rest, and anything else
        }
    }

    /// Split content into raw blocks by `###` markers, tracking 1-indexed lines.
    fn split_raw_blocks(content: &str) -> Vec<RawBlock> {
        let mut blocks = Vec::new();
        let mut current = String::new();
        let mut start_line: usize = 1;
        let mut line_no: usize = 0;

        for line in content.lines() {
            line_no += 1;
            if line.trim().starts_with("###") && !current.is_empty() {
                blocks.push(RawBlock {
                    start_line,
                    end_line: line_no - 1,
                    content: std::mem::take(&mut current),
                });
                start_line = line_no;
            }
            current.push_str(line);
            current.push('\n');
        }
        if !current.is_empty() {
            blocks.push(RawBlock {
                start_line,
                end_line: line_no.max(1),
                content: current,
            });
        }
        blocks
    }

    /// Parse a request file and extract the request at the given line.
    /// `file_ext` is the file extension (without dot), used for protocol detection.
    pub fn parse_at_line(&self, content: &str, line_num: usize, file_ext: &str) -> Result<Request> {
        let protocol = Self::detect_protocol(file_ext);
        let file_vars = self.extract_file_variables(content);
        let blocks = Self::split_raw_blocks(content);

        for block in &blocks {
            if line_num >= block.start_line && line_num <= block.end_line {
                return self.parse_block(&block.content, protocol, &file_vars);
            }
            // Fallback: cursor on inter-block separator falls into the next block
            // only when line_num matches start; otherwise use cumulative range like before.
        }

        // Preserve prior semantics: first block whose cumulative end >= line_num.
        let mut current_line = 0usize;
        for block in &blocks {
            let block_lines = block.content.lines().count();
            if current_line + block_lines >= line_num {
                return self.parse_block(&block.content, protocol, &file_vars);
            }
            current_line += block_lines;
        }

        anyhow::bail!("No request found at line {}", line_num);
    }

    /// Describe all request blocks in `content` as structured metadata.
    ///
    /// This is the single parse authority for block name/line/method/path/headers.
    /// Variable substitution uses the same rules as `parse_at_line`.
    pub fn describe_blocks(&self, content: &str, file_ext: &str) -> Result<Vec<BlockMeta>> {
        let protocol = Self::detect_protocol(file_ext);
        let file_vars = self.extract_file_variables(content);
        let raw_blocks = Self::split_raw_blocks(content);
        let mut out = Vec::with_capacity(raw_blocks.len());

        for raw in raw_blocks {
            // Skip pure preamble (file-level vars only, no ### and no request line)
            let has_separator = raw.content.lines().any(|l| l.trim().starts_with("###"));
            let request = match self.parse_block(&raw.content, protocol.clone(), &file_vars) {
                Ok(r) => r,
                Err(_) if !has_separator => continue, // file preamble without a request
                Err(e) => return Err(e),
            };

            let body_str = request.body_str();
            let (method, path, headers, body, request_line) =
                Self::extract_request_parts(body_str, &protocol);

            // File-level preamble parses as empty Request — skip unless it has a real request
            if request_line.is_empty() && !has_separator {
                continue;
            }

            out.push(BlockMeta {
                name: request.name.unwrap_or_default(),
                line: raw.start_line,
                end_line: raw.end_line,
                method,
                path,
                headers,
                body,
                request_line,
            });
        }

        Ok(out)
    }

    /// Split a resolved request body into method / path / headers / body parts.
    fn extract_request_parts(
        body: &str,
        protocol: &Protocol,
    ) -> (String, String, Vec<(String, String)>, String, String) {
        let lines: Vec<&str> = body.lines().collect();
        if lines.is_empty() {
            return (
                String::new(),
                String::new(),
                Vec::new(),
                String::new(),
                String::new(),
            );
        }

        // Find first non-empty non-comment line as the request line
        let mut idx = 0usize;
        while idx < lines.len() {
            let t = lines[idx].trim();
            if t.is_empty() || t.starts_with('#') {
                idx += 1;
                continue;
            }
            break;
        }
        if idx >= lines.len() {
            return (
                String::new(),
                String::new(),
                Vec::new(),
                String::new(),
                String::new(),
            );
        }

        let request_line = lines[idx].trim().to_string();
        let (method, path) = match request_line.split_once(char::is_whitespace) {
            Some((m, rest)) => (m.to_string(), rest.trim().to_string()),
            None => (request_line.clone(), String::new()),
        };

        let mut headers = Vec::new();
        let mut body_start = idx + 1;

        if matches!(protocol, Protocol::Http) {
            let mut i = idx + 1;
            while i < lines.len() {
                let t = lines[i].trim();
                if t.is_empty() {
                    body_start = i + 1;
                    break;
                }
                if let Some((k, v)) = lines[i].split_once(':') {
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                    body_start = i + 1;
                } else {
                    // Non-header line without blank separator — treat as body start
                    body_start = i;
                    break;
                }
                i += 1;
                if i == lines.len() {
                    body_start = i;
                }
            }
        } else {
            // Non-HTTP: everything after request line is body
            body_start = idx + 1;
        }

        let body = if body_start < lines.len() {
            lines[body_start..].join("\n")
        } else {
            String::new()
        };

        (method, path, headers, body, request_line)
    }

    fn parse_block(
        &self,
        block: &str,
        protocol: Protocol,
        file_vars: &HashMap<String, String>,
    ) -> Result<Request> {
        let lines: Vec<&str> = block.lines().collect();

        // First line should be ### Request Name
        let mut name = None;
        let mut request_lines = Vec::new();
        let mut request_vars = HashMap::new();
        let mut found_request_line = false;

        let mut in_assertion_block = false;
        let mut in_prescript_block = false;

        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim();
            i += 1;

            // Check for pre-request script block start: < {%
            if trimmed.starts_with("<") && trimmed.contains("{%") && !trimmed.contains("%}") {
                in_prescript_block = true;
                continue;
            }

            // Check for pre-request script block end: %}
            if in_prescript_block && trimmed == "%}" {
                in_prescript_block = false;
                continue;
            }

            // Skip lines inside pre-request script blocks
            if in_prescript_block {
                continue;
            }

            // Single-line pre-request script: < {% ... %}
            if trimmed.starts_with("<") && trimmed.contains("{%") && trimmed.contains("%}") {
                continue;
            }

            // External pre-request script: < ./path.lua
            if trimmed.starts_with("<")
                && (trimmed.contains("./") || trimmed.contains("../"))
                && trimmed.ends_with(".lua")
            {
                continue;
            }

            // Check for assertion block start: > {%
            if trimmed.starts_with(">") && trimmed.contains("{%") && !trimmed.contains("%}") {
                in_assertion_block = true;
                continue;
            }

            // Check for assertion block end: %}
            if in_assertion_block && trimmed == "%}" {
                in_assertion_block = false;
                continue;
            }

            // Skip lines inside assertion blocks
            if in_assertion_block {
                continue;
            }

            // Single-line assertion: > {% ... %}
            if trimmed.starts_with(">") && trimmed.contains("{%") && trimmed.contains("%}") {
                continue;
            }

            // External assertion script: > ./path.lua
            if trimmed.starts_with(">")
                && (trimmed.contains("./") || trimmed.contains("../"))
                && trimmed.ends_with(".lua")
            {
                continue;
            }

            if trimmed.starts_with("###") {
                // Extract name after ###
                name = Some(trimmed.trim_start_matches("###").trim().to_string());
            } else if !found_request_line {
                // Multi-line request-level var: @name=>>> ... <<<
                if let Some(var_name) = self.parse_multiline_var_start(line) {
                    let mut value_lines = Vec::new();
                    loop {
                        if i >= lines.len() {
                            break;
                        }
                        let next = lines[i];
                        i += 1;
                        if next.trim() == "<<<" {
                            break;
                        }
                        value_lines.push(next);
                    }
                    let raw_value = value_lines.join("\n");
                    let resolved = self.substitute_vars(&raw_value, file_vars, &request_vars);
                    request_vars.insert(var_name, resolved);
                    continue;
                }

                // Check if this is a variable definition before the request line
                if let Some((key, value)) = self.parse_variable_line(line) {
                    // Resolve {{var}} references using file-level and earlier request-level vars
                    let resolved = self.substitute_vars(&value, file_vars, &request_vars);
                    request_vars.insert(key, resolved);
                } else if !trimmed.is_empty()
                    && !trimmed.starts_with('#')
                    && !trimmed.starts_with('>')
                {
                    // This is the actual request line, mark it and add to request_lines
                    found_request_line = true;
                    request_lines.push(line);
                }
            } else {
                // After request line, add to request body (skip assertion markers and comments)
                if !trimmed.starts_with(">") && !trimmed.starts_with('#') {
                    request_lines.push(line);
                }
            }
        }

        // For HTTP, connection is embedded in the request line (URL)
        // For other protocols, we need @connection directive (block-level, then file-level fallback)
        let connection = match protocol {
            Protocol::Http => String::new(), // Will be extracted from request line
            _ => self
                .extract_connection(block, file_vars, &request_vars)
                .or_else(|_| {
                    // Fallback: look for @connection in file-level variables
                    file_vars.get("connection").cloned().ok_or_else(|| {
                        anyhow::anyhow!(
                            "No @connection directive found in request block or file header"
                        )
                    })
                })?,
        };

        // Reconstruct body without the ### line
        let body = self.substitute_vars(&request_lines.join("\n"), file_vars, &request_vars);

        Ok(Request {
            name,
            protocol,
            connection,
            body: body.into_bytes(),
            raw_body: String::new(), // filled by CLI after resolve_file_includes
        })
    }

    fn extract_connection(
        &self,
        block: &str,
        file_vars: &HashMap<String, String>,
        request_vars: &HashMap<String, String>,
    ) -> Result<String> {
        let re = Regex::new(r"(?:--|#)\s*@connection\s+(.+)")?;
        for line in block.lines() {
            if let Some(caps) = re.captures(line) {
                return Ok(self.substitute_vars(caps[1].trim(), file_vars, request_vars));
            }
        }
        anyhow::bail!("No @connection directive found in request block");
    }

    /// Parse a variable definition line in format `@name = value` or `@name value`
    fn parse_variable_line(&self, line: &str) -> Option<(String, String)> {
        let trimmed = line.trim();
        if !trimmed.starts_with('@') {
            return None;
        }

        let content = &trimmed[1..]; // Remove @

        // Try format: @name = value
        if let Some((name, value)) = content.split_once('=') {
            let name = name.trim().to_string();
            let mut value = value.trim().to_string();
            Self::strip_quotes(&mut value);
            if !name.is_empty() {
                return Some((name, value));
            }
        }

        // Try format: @name value
        if let Some((name, value)) = content.split_once(char::is_whitespace) {
            let name = name.trim().to_string();
            let mut value = value.trim().to_string();
            Self::strip_quotes(&mut value);
            if !name.is_empty() {
                return Some((name, value));
            }
        }

        None
    }

    /// Strip surrounding double or single quotes from a string value.
    fn strip_quotes(value: &mut String) {
        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            *value = value[1..value.len() - 1].to_string();
        }
    }

    /// Check if a line starts a multi-line variable definition (@name=>>> or @name >>>).
    /// Returns the variable name if a multi-line block starts here, None otherwise.
    fn parse_multiline_var_start(&self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        if !trimmed.starts_with('@') {
            return None;
        }

        let content = &trimmed[1..]; // Remove @

        // Format: @name=>>>  or  @name = >>>
        if let Some((name, marker)) = content.split_once('=') {
            if marker.trim() == ">>>" {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }

        // Format: @name >>>  (without equals sign)
        if let Some((name, marker)) = content.split_once(char::is_whitespace) {
            if marker.trim() == ">>>" {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }

        None
    }

    /// Extract file-level variables from content (before first ###).
    /// Supports:
    ///   - @name = value           — single-line, quoted values stripped
    ///   - @name value             — space-delimited
    ///   - @name=>>> ... <<<       — multi-line block value
    ///   - {{var}} references in values are resolved using earlier-defined vars.
    pub fn extract_file_variables(&self, content: &str) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;

        static CONNECTION_RE: OnceLock<Regex> = OnceLock::new();
        let connection_re = CONNECTION_RE.get_or_init(|| {
            Regex::new(r"(?:--|#)\s*@connection\s+(.+)").expect("valid literal regex: @connection")
        });

        while i < lines.len() {
            let line = lines[i];
            if line.trim().starts_with("###") {
                break; // Stop at first request
            }

            i += 1;

            // Multi-line var: @name=>>> ... <<<
            if let Some(name) = self.parse_multiline_var_start(line) {
                let mut value_lines = Vec::new();
                loop {
                    if i >= lines.len() {
                        break;
                    }
                    let next = lines[i];
                    i += 1;
                    if next.trim() == "<<<" {
                        break;
                    }
                    if next.trim().starts_with("###") {
                        i -= 1; // back up so outer loop sees ###
                        break;
                    }
                    value_lines.push(next);
                }
                let raw_value = value_lines.join("\n");
                let resolved = self.substitute_vars(&raw_value, &vars, &HashMap::new());
                vars.insert(name, resolved);
                continue;
            }

            if let Some((key, value)) = self.parse_variable_line(line) {
                // Resolve {{var}} references within the value using already-extracted vars
                let resolved = self.substitute_vars(&value, &vars, &HashMap::new());
                vars.insert(key, resolved);
            }

            // Also parse @connection directives in comments (# @connection ... or -- @connection ...)
            if let Some(caps) = connection_re.captures(line) {
                vars.insert("connection".to_string(), caps[1].trim().to_string());
            }
        }

        vars
    }

    fn substitute_vars(
        &self,
        input: &str,
        file_vars: &HashMap<String, String>,
        request_vars: &HashMap<String, String>,
    ) -> String {
        let resolver = VarResolver::new()
            .with_request_vars(request_vars.clone())
            .with_file_vars(file_vars.clone())
            .with_env(self.env.clone());
        resolver.substitute(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Parser existing tests ----

    #[test]
    fn test_substitute_vars_simple() {
        let mut env_vars = HashMap::new();
        env_vars.insert("name".to_string(), "John".to_string());
        let parser = Parser::new(env_vars);
        let result = parser.substitute_vars("Hello, {{name}}!", &HashMap::new(), &HashMap::new());
        assert_eq!(result, "Hello, John!");
    }

    #[test]
    fn test_substitute_vars_multiple() {
        let mut env_vars = HashMap::new();
        env_vars.insert("first".to_string(), "Jane".to_string());
        env_vars.insert("last".to_string(), "Doe".to_string());
        let parser = Parser::new(env_vars);
        let result = parser.substitute_vars("{{first}} {{last}}", &HashMap::new(), &HashMap::new());
        assert_eq!(result, "Jane Doe");
    }

    #[test]
    fn test_substitute_vars_not_found() {
        let parser = Parser::new(HashMap::new());
        let result = parser.substitute_vars("{{missing}}", &HashMap::new(), &HashMap::new());
        assert_eq!(result, "{{missing}}");
    }

    #[test]
    fn test_substitute_vars_no_vars() {
        let parser = Parser::new(HashMap::new());
        let result = parser.substitute_vars("no variables", &HashMap::new(), &HashMap::new());
        assert_eq!(result, "no variables");
    }

    #[test]
    fn test_extract_connection_success() {
        let parser = Parser::new(HashMap::new());
        let block = "# @connection redis://localhost:6379\nGET user:123";
        let result = parser
            .extract_connection(block, &HashMap::new(), &HashMap::new())
            .unwrap();
        assert_eq!(result, "redis://localhost:6379");
    }

    #[test]
    fn test_extract_connection_missing() {
        let parser = Parser::new(HashMap::new());
        let block = "GET http://example.com";
        let result = parser.extract_connection(block, &HashMap::new(), &HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_connection_postgres() {
        let parser = Parser::new(HashMap::new());
        let block = "# @connection postgres://user:pass@localhost:5432/db\nSELECT 1";
        let result = parser
            .extract_connection(block, &HashMap::new(), &HashMap::new())
            .unwrap();
        assert_eq!(result, "postgres://user:pass@localhost:5432/db");
    }

    #[test]
    fn test_extract_connection_with_vars() {
        let mut env_vars = HashMap::new();
        env_vars.insert("db_host".to_string(), "localhost".to_string());
        env_vars.insert("db_port".to_string(), "5432".to_string());
        let parser = Parser::new(env_vars);
        let block = "# @connection postgres://user:pass@{{db_host}}:{{db_port}}/mydb\nSELECT 1";
        let result = parser
            .extract_connection(block, &HashMap::new(), &HashMap::new())
            .unwrap();
        assert_eq!(result, "postgres://user:pass@localhost:5432/mydb");
    }

    #[test]
    fn test_parse_variable_line_equals() {
        let parser = Parser::new(HashMap::new());
        let result = parser.parse_variable_line("@host = https://example.com");
        assert_eq!(
            result,
            Some(("host".to_string(), "https://example.com".to_string()))
        );
    }

    #[test]
    fn test_parse_variable_line_space() {
        let parser = Parser::new(HashMap::new());
        let result = parser.parse_variable_line("@host https://example.com");
        assert_eq!(
            result,
            Some(("host".to_string(), "https://example.com".to_string()))
        );
    }

    #[test]
    fn test_parse_variable_line_invalid() {
        let parser = Parser::new(HashMap::new());
        assert_eq!(parser.parse_variable_line("GET https://example.com"), None);
        assert_eq!(parser.parse_variable_line("# comment"), None);
        assert_eq!(parser.parse_variable_line("@host"), None);
    }

    #[test]
    fn test_extract_file_variables() {
        let parser = Parser::new(HashMap::new());
        let content = r#"@host = https://api.example.com
@token = abc123

### Request 1
GET {{host}}/users
"#;
        let vars = parser.extract_file_variables(content);
        assert_eq!(
            vars.get("host"),
            Some(&"https://api.example.com".to_string())
        );
        assert_eq!(vars.get("token"), Some(&"abc123".to_string()));
    }

    #[test]
    fn test_file_variables_stop_at_request() {
        let parser = Parser::new(HashMap::new());
        let content = r#"@host = https://api.example.com

### Request 1
@should_not_parse = value
GET /users
"#;
        let vars = parser.extract_file_variables(content);
        assert_eq!(vars.len(), 1);
        assert_eq!(
            vars.get("host"),
            Some(&"https://api.example.com".to_string())
        );
        assert_eq!(vars.get("should_not_parse"), None);
    }

    #[test]
    fn test_request_variables() {
        let parser = Parser::new(HashMap::new());
        let block = r#"### Request 1
@user_id = 123
@api_key = secret
GET /users/{{user_id}}
Authorization: Bearer {{api_key}}
"#;
        let request = parser
            .parse_block(block, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(request.body_str().contains("GET /users/123"));
        assert!(request.body_str().contains("Authorization: Bearer secret"));
    }

    #[test]
    fn test_variable_priority_integration() {
        let mut env_vars = HashMap::new();
        env_vars.insert("host".to_string(), "env.com".to_string());
        env_vars.insert("port".to_string(), "8080".to_string());

        let parser = Parser::new(env_vars);

        let content = r#"@host = file.com
@timeout = 30

### Request 1
@host = request.com
GET http://{{host}}:{{port}}/{{timeout}}
"#;

        let request = parser.parse_at_line(content, 6, "http").unwrap();

        // host should be from request_vars (highest priority)
        assert!(request.body_str().contains("http://request.com:8080/30"));
    }

    #[test]
    fn test_prescript_multiline_stripped() {
        let parser = Parser::new(HashMap::new());
        let block = "### Request 1\n< {%\n  local x = 1\n%}\nGET /api/data\n";
        let request = parser
            .parse_block(block, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(request.body_str().contains("GET /api/data"));
        assert!(!request.body_str().contains("{%"));
        assert!(!request.body_str().contains("local x"));
    }

    #[test]
    fn test_prescript_singleline_stripped() {
        let parser = Parser::new(HashMap::new());
        let block =
            "### Request 1\n< {% request.variables.set(\"token\", \"abc\") %}\nGET /api/data\n";
        let request = parser
            .parse_block(block, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(request.body_str().contains("GET /api/data"));
        assert!(!request.body_str().contains("{%"));
    }

    #[test]
    fn test_prescript_external_stripped() {
        let parser = Parser::new(HashMap::new());
        let block = "### Request 1\n< ./scripts/gen.lua\nGET /api/data\n";
        let request = parser
            .parse_block(block, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(request.body_str().contains("GET /api/data"));
        assert!(!request.body_str().contains("gen.lua"));
    }

    #[test]
    fn test_assertion_external_stripped() {
        let parser = Parser::new(HashMap::new());
        let block = "### Request 1\nGET /api/data\n> ./scripts/check.lua\n";
        let request = parser
            .parse_block(block, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(request.body_str().contains("GET /api/data"));
        assert!(!request.body_str().contains("check.lua"));
    }

    #[test]
    fn test_assertion_external_stripped_multi_block() {
        let parser = Parser::new(HashMap::new());
        let content = "### Request 1\nGET /api/data\n> ./scripts/check.lua\n\n### Request 2\nGET /api/other\n";
        let request = parser.parse_at_line(content, 2, "http").unwrap();
        assert!(request.body_str().contains("GET /api/data"));
        assert!(!request.body_str().contains("check.lua"));
    }

    #[test]
    fn test_prescript_injected_vars() {
        let parser = Parser::new(HashMap::new());
        let block = "### Request 1\n@auth_token = injected-value\nGET /api?token={{auth_token}}\n";
        let request = parser
            .parse_block(block, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(request.body_str().contains("GET /api?token=injected-value"));
    }

    // ---- @var enhancements: quote stripping, {{var}} in values, multi-line blocks ----

    #[test]
    fn test_parse_variable_line_quotes_stripped() {
        let parser = Parser::new(HashMap::new());
        let r = parser.parse_variable_line("@host = \"https://example.com\"");
        assert_eq!(
            r,
            Some(("host".to_string(), "https://example.com".to_string()))
        );
    }

    #[test]
    fn test_parse_variable_line_single_quotes_stripped() {
        let parser = Parser::new(HashMap::new());
        let r = parser.parse_variable_line("@host 'http://localhost'");
        assert_eq!(
            r,
            Some(("host".to_string(), "http://localhost".to_string()))
        );
    }

    #[test]
    fn test_file_var_references_other_file_var() {
        let parser = Parser::new(HashMap::new());
        let content = r#"@pageNum = 1
@pageSize = 10
@page = pageNum={{pageNum}}&pageSize={{pageSize}}

### Request
GET /api?{{page}}
"#;
        let vars = parser.extract_file_variables(content);
        assert_eq!(vars.get("pageNum"), Some(&"1".to_string()));
        assert_eq!(vars.get("pageSize"), Some(&"10".to_string()));
        assert_eq!(vars.get("page"), Some(&"pageNum=1&pageSize=10".to_string()));

        let req = parser.parse_at_line(content, 5, "http").unwrap();
        assert!(req.body_str().contains("GET /api?pageNum=1&pageSize=10"));
    }

    #[test]
    fn test_multiline_file_var() {
        let parser = Parser::new(HashMap::new());
        let content = r#"@token = abc123
@headers=>>>
Authorization: {{token}}
X-Custom: yes
<<<

### Request
POST /api/data
{{headers}}

{"key": "value"}
"#;
        let req = parser.parse_at_line(content, 8, "http").unwrap();
        assert!(req.body_str().contains("Authorization: abc123"));
        assert!(req.body_str().contains("X-Custom: yes"));
        assert!(req.body_str().contains("{\"key\": \"value\"}"));
    }

    #[test]
    fn test_multiline_file_var_forward_ref_resolved() {
        let parser = Parser::new(HashMap::new());
        // Iterative substitution resolves even forward references
        let content = r#"@page = id={{pageNum}}
@pageNum = 99

### Request
GET /{{page}}
"#;
        let req = parser.parse_at_line(content, 4, "http").unwrap();
        assert!(req.body_str().contains("GET /id=99"));
        assert!(!req.body_str().contains("{{pageNum}}"));
    }

    #[test]
    fn test_request_var_refers_to_file_var() {
        let parser = Parser::new(HashMap::new());
        let content = r#"@base = /api/v1

### Request
@path = {{base}}/users
GET {{path}}
"#;
        let req = parser.parse_at_line(content, 5, "http").unwrap();
        assert!(req.body_str().contains("GET /api/v1/users"));
    }

    #[test]
    fn test_multiline_request_var() {
        let parser = Parser::new(HashMap::new());
        let block = r#"### Request
@token = secret
@headers=>>>
Authorization: {{token}}
Content-Type: application/json
<<<
POST /api/data
{{headers}}
"#;
        let req = parser
            .parse_block(block, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(req.body_str().contains("POST /api/data"));
        assert!(req.body_str().contains("Authorization: secret"));
        assert!(req.body_str().contains("Content-Type: application/json"));
    }

    #[test]
    fn test_var_transitive_resolution_file_level() {
        let parser = Parser::new(HashMap::new());
        let content = r#"@admin_token = secret
@token = {{admin_token}}

### Request
GET /api
Authorization: {{token}}
"#;
        let req = parser.parse_at_line(content, 5, "http").unwrap();
        assert!(req.body_str().contains("Authorization: secret"));
        assert!(!req.body_str().contains("{{admin_token}}"));
    }

    #[test]
    fn test_var_transitive_resolution_forward_ref() {
        let parser = Parser::new(HashMap::new());
        // admin_token defined AFTER token at file level.
        // extract_file_variables can't resolve token at definition time,
        // but the body-level iterative substitution resolves the full chain.
        let content = r#"@token = {{admin_token}}
@admin_token = secret

### Request
GET /api
Authorization: {{token}}
"#;
        let req = parser.parse_at_line(content, 5, "http").unwrap();
        assert!(req.body_str().contains("Authorization: secret"));
        assert!(!req.body_str().contains("{{admin_token}}"));
    }

    #[test]
    fn test_var_transitive_resolution_request_level() {
        let parser = Parser::new(HashMap::new());
        let block = r#"### Request
@admin_token = secret
@token = {{admin_token}}
GET /api
Authorization: {{token}}
"#;
        let req = parser
            .parse_block(block, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(req.body_str().contains("Authorization: secret"));
    }

    #[test]
    fn test_magic_var_timestamp() {
        let parser = Parser::new(HashMap::new());
        let result = parser.substitute_vars("{{$timestamp}}", &HashMap::new(), &HashMap::new());
        // Should be a long numeric string (timestamp + random)
        assert!(result.len() > 10);
        assert!(result.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_magic_var_uuid() {
        let parser = Parser::new(HashMap::new());
        let result = parser.substitute_vars("{{$uuid}}", &HashMap::new(), &HashMap::new());
        // UUID format: 8-4-4-4-12
        assert_eq!(result.len(), 36);
        assert_eq!(result.chars().filter(|&c| c == '-').count(), 4);
    }

    #[test]
    fn test_magic_var_date() {
        let parser = Parser::new(HashMap::new());
        let result = parser.substitute_vars("{{$date}}", &HashMap::new(), &HashMap::new());
        // YYYY-MM-DD
        assert_eq!(result.len(), 10);
        assert_eq!(&result[4..5], "-");
        assert_eq!(&result[7..8], "-");
    }

    #[test]
    fn test_magic_var_randomInt() {
        let parser = Parser::new(HashMap::new());
        let result = parser.substitute_vars("{{$randomInt}}", &HashMap::new(), &HashMap::new());
        let val: u64 = result.parse().unwrap();
        assert!(val < 10000000);
    }

    #[test]
    fn test_magic_var_not_found_preserved() {
        let parser = Parser::new(HashMap::new());
        let result = parser.substitute_vars("{{$unknown}}", &HashMap::new(), &HashMap::new());
        assert_eq!(result, "{{$unknown}}");
    }

    #[test]
    fn test_comments_between_blocks_excluded_from_body() {
        let parser = Parser::new(HashMap::new());
        let content = "### Request 1\nGET /api/one\n\n> {% client.test(\"a\", function() end) %}\n\n# ─────────────────\n# Comment between blocks\n# ─────────────────\n\n### Request 2\nGET /api/two\n";
        let request = parser.parse_at_line(content, 2, "http").unwrap();
        assert!(request.body_str().contains("GET /api/one"));
        assert!(
            !request.body_str().contains("Comment between blocks"),
            "body should not contain inter-block comments"
        );
        assert!(
            !request.body_str().contains("──"),
            "body should not contain inter-block comment decorations"
        );
    }

    #[test]
    fn test_magic_var_in_body() {
        let parser = Parser::new(HashMap::new());
        let content = "### Request\nPOST /api/log\nContent-Type: application/json\n\n{\"ts\": \"{{$timestamp}}\", \"uuid\": \"{{$uuid}}\"}\n";
        let request = parser
            .parse_block(content, Protocol::Http, &HashMap::new())
            .unwrap();
        assert!(!request.body_str().contains("{{$timestamp}}"));
        assert!(!request.body_str().contains("{{$uuid}}"));
        assert!(request.body_str().contains("\"ts\": \""));
        assert!(request.body_str().contains("\"uuid\": \""));
    }

    #[test]
    fn test_var_circular_ref_no_infinite_loop() {
        let parser = Parser::new(HashMap::new());
        let content = r#"@a = {{b}}
@b = {{a}}

### Request
GET /api
X-Val: {{a}}
"#;
        // Should not hang or panic — caps at 20 iterations
        let req = parser.parse_at_line(content, 5, "http").unwrap();
        let body = req.body_str();
        assert!(body.contains("X-Val: {{b}}") || body.contains("X-Val: {{a}}"));
    }

    // ---- describe_blocks (Phase 2 single parse authority) ----

    #[test]
    fn test_describe_blocks_basic() {
        let parser = Parser::new(HashMap::new());
        let content = r#"@base = http://example.com

### Get Users
GET {{base}}/api/users
Accept: application/json

### Create User
POST {{base}}/api/users
Content-Type: application/json

{"name": "Ada"}
"#;
        let blocks = parser.describe_blocks(content, "http").unwrap();
        assert_eq!(blocks.len(), 2);

        assert_eq!(blocks[0].name, "Get Users");
        assert_eq!(blocks[0].line, 3);
        assert_eq!(blocks[0].method, "GET");
        assert_eq!(blocks[0].path, "http://example.com/api/users");
        assert_eq!(blocks[0].headers.len(), 1);
        assert_eq!(blocks[0].headers[0].0, "Accept");
        assert_eq!(blocks[0].headers[0].1, "application/json");
        assert_eq!(blocks[0].request_line, "GET http://example.com/api/users");

        assert_eq!(blocks[1].name, "Create User");
        assert_eq!(blocks[1].method, "POST");
        assert!(blocks[1].body.contains(r#""name": "Ada""#));
        assert_eq!(blocks[1].headers[0].0, "Content-Type");
    }

    #[test]
    fn test_describe_blocks_line_numbers() {
        let parser = Parser::new(HashMap::new());
        let content = "### A\nGET /a\n\n### B\nGET /b\n";
        let blocks = parser.describe_blocks(content, "http").unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].line, 1);
        assert_eq!(blocks[0].end_line, 3); // includes blank line before next ###
        assert_eq!(blocks[1].line, 4);
        assert_eq!(blocks[1].method, "GET");
        assert_eq!(blocks[1].path, "/b");
    }

    #[test]
    fn test_describe_blocks_skips_preamble_only() {
        let parser = Parser::new(HashMap::new());
        // Content with only file-level vars and no ### — no blocks
        let content = "@x = 1\n@y = 2\n";
        let blocks = parser.describe_blocks(content, "http").unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_describe_blocks_script() {
        let parser = Parser::new(HashMap::new());
        let content = "### Setup\nSCRIPT\n";
        let blocks = parser.describe_blocks(content, "http").unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].method, "SCRIPT");
    }
}
