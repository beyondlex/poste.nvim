use anyhow::{Context, Result};
use clap::Parser;
use poste_core::VarResolver;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

/// Resolve variables in a .http request block
#[derive(Parser)]
pub struct ResolveArgs {
    /// .http file path
    #[arg(short, long)]
    pub file: PathBuf,

    /// Target ### block line number
    #[arg(short, long)]
    pub block: usize,

    /// Resolve a single variable (for K key lookup)
    #[arg(long)]
    pub var: Option<String>,

    /// Output format: value | content | verbose | curl
    #[arg(long, default_value = "value")]
    pub format: String,

    /// Import parameters as JSON: {"key": "value"}
    #[arg(long)]
    pub import_params: Option<String>,

    /// Session/global variables as JSON: {"key": "value"}
    #[arg(long)]
    pub session_vars: Option<String>,

    /// Script variables as JSON: {"key": "value"}
    #[arg(long)]
    pub script_vars: Option<String>,

    /// Environment name (for env.json lookup)
    #[arg(short, long, default_value = "dev")]
    pub env: String,

    /// Read request content from stdin instead of from the file
    #[arg(long)]
    pub stdin: bool,
}

pub fn execute(args: ResolveArgs) -> Result<()> {
    // Read the content (from stdin or file)
    let content = if args.stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        std::fs::read_to_string(&args.file)
            .with_context(|| format!("Cannot read file: {}", args.file.display()))?
    };

    // Determine the search directory for env.json
    let search_dir = args
        .file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Load env.json variables
    let env_vars = load_env_vars(&search_dir, &args.env);

    // Parse import params from JSON
    let import_params = args
        .import_params
        .as_deref()
        .map(parse_json_vars)
        .transpose()?
        .unwrap_or_default();

    // Parse session vars from JSON
    let session_vars = args
        .session_vars
        .as_deref()
        .map(parse_json_vars)
        .transpose()?
        .unwrap_or_default();

    // Parse script vars from JSON
    let script_vars = args
        .script_vars
        .as_deref()
        .map(parse_json_vars)
        .transpose()?
        .unwrap_or_default();

    // Extract file-level variables (before first ###)
    let file_vars = extract_file_variables(&content);

    // Extract request-level variables from the target block
    let request_vars = extract_request_variables(&content, args.block);

    // Build the resolver with all layers
    let resolver = VarResolver::new()
        .with_import_params(import_params)
        .with_request_vars(request_vars)
        .with_file_vars(file_vars)
        .with_session_vars(session_vars)
        .with_script_vars(script_vars)
        .with_env(env_vars);

    // If --var is specified, resolve just that single variable
    if let Some(var_name) = &args.var {
        match resolver.resolve(var_name) {
            Some(value) => {
                if args.format == "verbose" {
                    println!("{{\"value\": \"{}\", \"source\": \"resolved\"}}", value);
                } else {
                    println!("{}", value);
                }
            }
            None => {
                if args.format != "verbose" {
                    println!();
                } else {
                    println!("{{\"value\": null, \"source\": \"unresolved\"}}");
                }
            }
        }
        return Ok(());
    }

    // Otherwise, resolve the full request content
    match args.format.as_str() {
        "content" | "verbose" => {
            let file_ext = args
                .file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("http");
            let resolved = resolve_request_content(&content, args.block, &resolver, file_ext);
            println!("{}", resolved);
        }
        "curl" => {
            let file_ext = args
                .file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("http");
            let resolved = resolve_request_content(&content, args.block, &resolver, file_ext);
            let curl = format_as_curl(&resolved);
            println!("{}", curl);
        }
        "value" => {
            let file_ext = args
                .file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("http");
            let resolved = resolve_request_content(&content, args.block, &resolver, file_ext);
            println!("{}", resolved);
        }
        other => {
            anyhow::bail!(
                "Unknown format: {}. Use: value, content, verbose, or curl",
                other
            );
        }
    }

    Ok(())
}

/// Parse a JSON string into a HashMap<String, String>.
fn parse_json_vars(json_str: &str) -> Result<HashMap<String, String>> {
    let raw: HashMap<String, serde_json::Value> = serde_json::from_str(json_str)?;
    let mut vars = HashMap::new();
    for (k, v) in raw {
        let val = match v {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        };
        vars.insert(k, val);
    }
    Ok(vars)
}

/// Load env vars for variable substitution.
fn load_env_vars(search_dir: &std::path::Path, env_name: &str) -> HashMap<String, String> {
    let mut dir = search_dir;
    loop {
        let candidate = dir.join("env.json");
        if candidate.exists() {
            if let Ok(env_file) = poste_core::Environment::load(
                candidate
                    .to_str()
                    .expect("env.json path must be valid UTF-8"),
            ) {
                if let Some(vars) = env_file.envs.get(env_name) {
                    return vars.clone();
                }
            }
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    HashMap::new()
}

/// Extract file-level variables from content (before first ###).
fn extract_file_variables(content: &str) -> HashMap<String, String> {
    let parser = poste_core::Parser::new(HashMap::new());
    parser.extract_file_variables(content)
}

/// Extract request-level variables from the target block.
fn extract_request_variables(content: &str, block_line: usize) -> HashMap<String, String> {
    let blocks = split_into_blocks(content);
    let mut current_line = 0;
    for block in &blocks {
        let block_lines = block.lines().count();
        if current_line + block_lines >= block_line {
            return extract_vars_from_block(block);
        }
        current_line += block_lines;
    }
    HashMap::new()
}

/// Split content into blocks by ### markers.
fn split_into_blocks(content: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current_block = String::new();
    for line in content.lines() {
        if line.trim().starts_with("###") && !current_block.is_empty() {
            blocks.push(current_block.clone());
            current_block.clear();
        }
        current_block.push_str(line);
        current_block.push('\n');
    }
    if !current_block.is_empty() {
        blocks.push(current_block);
    }
    blocks
}

/// Extract variable definitions from a block (before the request line).
fn extract_vars_from_block(block: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let mut found_request_line = false;

    for line in block.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("###") {
            continue;
        }

        if found_request_line {
            break;
        }

        if trimmed.starts_with('@') {
            if let Some((key, value)) = parse_variable_line(line) {
                vars.insert(key, value);
            }
        } else if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with('>')
            && !trimmed.starts_with('<')
        {
            // This is the request line — stop collecting vars
            found_request_line = true;
        }
    }

    vars
}

/// Parse a variable definition line in format `@name = value` or `@name value`.
fn parse_variable_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with('@') {
        return None;
    }

    let content = &trimmed[1..]; // Remove @

    // Try format: @name = value
    if let Some((name, value)) = content.split_once('=') {
        let name = name.trim().to_string();
        let mut value = value.trim().to_string();
        strip_quotes(&mut value);
        if !name.is_empty() {
            return Some((name, value));
        }
    }

    // Try format: @name value
    if let Some((name, value)) = content.split_once(char::is_whitespace) {
        let name = name.trim().to_string();
        let mut value = value.trim().to_string();
        strip_quotes(&mut value);
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

/// Resolve variables in the request content for the given block.
fn resolve_request_content(
    content: &str,
    block_line: usize,
    resolver: &VarResolver,
    _file_ext: &str,
) -> String {
    let blocks = split_into_blocks(content);
    let mut current_line = 0;
    for block in &blocks {
        let block_lines = block.lines().count();
        if current_line + block_lines >= block_line {
            return resolver.substitute(block);
        }
        current_line += block_lines;
    }
    resolver.substitute(content)
}

/// Format a resolved request as a curl command.
fn format_as_curl(resolved: &str) -> String {
    let lines: Vec<&str> = resolved.lines().collect();
    let mut method = "GET";
    let mut url = String::new();
    let mut headers = Vec::new();
    let mut body = String::new();
    let mut state = "preamble"; // "preamble" | "request-line" | "headers" | "body"

    for line in &lines {
        let trimmed = line.trim();

        if trimmed.starts_with("###") {
            continue;
        }

        match state {
            "preamble" => {
                // Skip @var lines and empty lines before the request
                if trimmed.starts_with('@') || trimmed.is_empty() {
                    continue;
                }
                // Check if this looks like a request line
                if is_request_line(trimmed) {
                    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
                    method = parts[0];
                    url = if parts.len() == 2 {
                        parts[1].to_string()
                    } else {
                        parts[0].to_string()
                    };
                    state = "headers";
                }
            }
            "headers" => {
                if trimmed.is_empty() {
                    state = "body";
                } else if trimmed.contains(": ") {
                    headers.push(trimmed);
                } else {
                    // Not a header and not empty — assume body started
                    body.push_str(trimmed);
                    body.push('\n');
                    state = "body";
                }
            }
            "body" => {
                if !trimmed.is_empty() {
                    body.push_str(trimmed);
                    body.push('\n');
                }
            }
            _ => {}
        }
    }

    let mut curl = format!("curl -X {}", method);
    if !url.is_empty() {
        curl.push_str(&format!(" '{}'", url));
    }
    for header in &headers {
        if let Some((key, val)) = header.split_once(": ") {
            curl.push_str(&format!(" -H '{}: {}'", key, val));
        }
    }
    let body = body.trim();
    if !body.is_empty() {
        curl.push_str(&format!(" -d '{}'", body));
    }

    curl
}

/// Check if a trimmed line looks like an HTTP request line.
fn is_request_line(trimmed: &str) -> bool {
    trimmed.contains("://")
        || trimmed.starts_with("GET ")
        || trimmed.starts_with("POST ")
        || trimmed.starts_with("PUT ")
        || trimmed.starts_with("DELETE ")
        || trimmed.starts_with("PATCH ")
        || trimmed.starts_with("HEAD ")
        || trimmed.starts_with("OPTIONS ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_vars() {
        let vars = parse_json_vars(r#"{"key": "value", "num": 42}"#).unwrap();
        assert_eq!(vars.get("key"), Some(&"value".to_string()));
        assert_eq!(vars.get("num"), Some(&"42".to_string()));
    }

    #[test]
    fn test_parse_json_vars_empty() {
        let vars = parse_json_vars("{}").unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    fn test_parse_json_vars_invalid() {
        assert!(parse_json_vars("not json").is_err());
    }

    #[test]
    fn test_extract_file_variables() {
        let content = r#"@host = https://api.example.com
@token = abc123

### Request 1
GET {{host}}/users
"#;
        let vars = extract_file_variables(content);
        assert_eq!(
            vars.get("host"),
            Some(&"https://api.example.com".to_string())
        );
        assert_eq!(vars.get("token"), Some(&"abc123".to_string()));
    }

    #[test]
    fn test_extract_request_variables() {
        let content = r#"@host = https://api.example.com

### Login
@user_token = injected
POST {{host}}/login
"#;
        let vars = extract_request_variables(content, 4);
        assert_eq!(vars.get("user_token"), Some(&"injected".to_string()));
    }

    #[test]
    fn test_parse_variable_line_equals() {
        let r = parse_variable_line("@host = https://example.com");
        assert_eq!(
            r,
            Some(("host".to_string(), "https://example.com".to_string()))
        );
    }

    #[test]
    fn test_parse_variable_line_space() {
        let r = parse_variable_line("@host https://example.com");
        assert_eq!(
            r,
            Some(("host".to_string(), "https://example.com".to_string()))
        );
    }

    #[test]
    fn test_parse_variable_line_invalid() {
        assert_eq!(parse_variable_line("GET https://example.com"), None);
        assert_eq!(parse_variable_line("# comment"), None);
    }

    #[test]
    fn test_format_as_curl_get() {
        let input = "GET http://example.com/api\nAuthorization: Bearer token123\n";
        let curl = format_as_curl(input);
        assert!(curl.contains("curl -X GET"));
        assert!(curl.contains("http://example.com/api"));
        assert!(curl.contains("Authorization: Bearer token123"));
        // Header should be -H, not -d
        assert!(curl.contains("-H"));
        assert!(!curl.contains("-d"));
    }

    #[test]
    fn test_format_as_curl_post() {
        let input =
            "POST http://example.com/api\nContent-Type: application/json\n\n{\"key\": \"value\"}\n";
        let curl = format_as_curl(input);
        assert!(curl.contains("curl -X POST"));
        assert!(curl.contains("-d"));
        assert!(curl.contains("{\"key\": \"value\"}"));
        assert!(curl.contains("-H 'Content-Type: application/json'"));
    }
}
