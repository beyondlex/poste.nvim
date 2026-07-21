use super::*;

// ─── Tokenizer tests ───

#[test]
fn test_tokenize_import() {
    let regions = Tokenizer::tokenize("import ./auth.http\nimport ./orders.http as orders\n");
    assert_eq!(regions.len(), 2);
    assert_eq!(
        regions[0],
        Region::Import {
            path: "./auth.http".to_string(),
            alias: None,
            raw: "import ./auth.http".to_string(),
        }
    );
    assert_eq!(
        regions[1],
        Region::Import {
            path: "./orders.http".to_string(),
            alias: Some("orders".to_string()),
            raw: "import ./orders.http as orders".to_string(),
        }
    );
}

#[test]
fn test_tokenize_run() {
    let regions = Tokenizer::tokenize("run #Login\nrun #orders.ListOrders (@token=xyz)\n");
    assert_eq!(regions.len(), 2);
    assert_eq!(
        regions[0],
        Region::Run {
            target: "#Login".to_string(),
            raw: "run #Login".to_string(),
        }
    );
}

#[test]
fn test_tokenize_separator() {
    let regions = Tokenizer::tokenize("### Get users\n");
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0], Region::Separator("### Get users".to_string()));
}

#[test]
fn test_tokenize_vardef_simple() {
    let regions = Tokenizer::tokenize("@base_url = https://api.example.com\n");
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::VarDef {
            name, value, style, ..
        } => {
            assert_eq!(name, "base_url");
            assert_eq!(value, "https://api.example.com");
            assert_eq!(*style, VarStyle::Simple);
        }
        _ => panic!("Expected VarDef"),
    }
}

#[test]
fn test_tokenize_vardef_multiline() {
    let content = "@payload =>>>\n{\n  \"name\": \"test\"\n}\n<<<\n";
    let regions = Tokenizer::tokenize(content);
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::VarDef {
            name, value, style, ..
        } => {
            assert_eq!(name, "payload");
            assert_eq!(value, "{\n  \"name\": \"test\"\n}");
            assert_eq!(
                *style,
                VarStyle::Multiline {
                    terminator: "<<<".to_string()
                }
            );
        }
        _ => panic!("Expected VarDef"),
    }
}

#[test]
fn test_tokenize_request_line() {
    let regions = Tokenizer::tokenize("GET https://api.example.com/users\n");
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::RequestLine {
            method,
            url,
            version,
            ..
        } => {
            assert_eq!(method, "GET");
            assert_eq!(url, "https://api.example.com/users");
            assert_eq!(*version, None);
        }
        _ => panic!("Expected RequestLine"),
    }
}

#[test]
fn test_tokenize_request_line_with_version() {
    let regions = Tokenizer::tokenize("POST https://api.example.com/data HTTP/1.1\n");
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::RequestLine {
            method,
            url,
            version,
            ..
        } => {
            assert_eq!(method, "POST");
            assert_eq!(url, "https://api.example.com/data");
            assert_eq!(*version, Some("HTTP/1.1".to_string()));
        }
        _ => panic!("Expected RequestLine"),
    }
}

#[test]
fn test_tokenize_header() {
    let regions =
        Tokenizer::tokenize("Content-Type: application/json\nAuthorization: Bearer token\n");
    assert_eq!(regions.len(), 2);
    match &regions[0] {
        Region::Header { key, value, .. } => {
            assert_eq!(key, "Content-Type");
            assert_eq!(value, "application/json");
        }
        _ => panic!("Expected Header"),
    }
}

#[test]
fn test_tokenize_comment() {
    let regions = Tokenizer::tokenize("# This is a comment\n");
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::Comment(text) => assert_eq!(text, " This is a comment"),
        _ => panic!("Expected Comment"),
    }
}

#[test]
fn test_tokenize_prescript_inline() {
    let regions = Tokenizer::tokenize("< {% client.log(\"pre\"); %}\n");
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::PreScript { code, style } => {
            assert_eq!(code, "client.log(\"pre\");");
            assert!(matches!(style, ScriptStyle::Inline(_)));
        }
        _ => panic!("Expected PreScript"),
    }
}

#[test]
fn test_tokenize_prescript_multiline() {
    let content = "< {%\n  local x = 1\n  client.log(x)\n%}\n";
    let regions = Tokenizer::tokenize(content);
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::PreScript { code, style } => {
            assert_eq!(code, "  local x = 1\n  client.log(x)");
            assert!(matches!(style, ScriptStyle::Multiline(_)));
        }
        _ => panic!("Expected PreScript"),
    }
}

#[test]
fn test_tokenize_postscript_inline() {
    let regions = Tokenizer::tokenize("> {% client.assert(response.status == 200); %}\n");
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::PostScript { code, .. } => {
            assert_eq!(code, "client.assert(response.status == 200);");
        }
        _ => panic!("Expected PostScript"),
    }
}

#[test]
fn test_tokenize_postscript_multiline() {
    let content = "> {%\n  client.test(\"ok\", function()\n    client.assert(true)\n  end)\n%}\n";
    let regions = Tokenizer::tokenize(content);
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::PostScript { code, style } => {
            assert!(code.contains("client.test"));
            assert!(matches!(style, ScriptStyle::Multiline(_)));
        }
        _ => panic!("Expected PostScript"),
    }
}

#[test]
fn test_tokenize_external_script_pre() {
    let regions = Tokenizer::tokenize("< ./scripts/gen.lua\n");
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::ExternalScript { path, script_type } => {
            assert_eq!(path, "./scripts/gen.lua");
            assert_eq!(*script_type, ScriptType::Pre);
        }
        _ => panic!("Expected ExternalScript"),
    }
}

#[test]
fn test_tokenize_external_script_post() {
    let regions = Tokenizer::tokenize("> ./scripts/check.lua\n");
    assert_eq!(regions.len(), 1);
    match &regions[0] {
        Region::ExternalScript { path, script_type } => {
            assert_eq!(path, "./scripts/check.lua");
            assert_eq!(*script_type, ScriptType::Post);
        }
        _ => panic!("Expected ExternalScript"),
    }
}

#[test]
fn test_tokenize_blank_line() {
    let regions = Tokenizer::tokenize("\n");
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0], Region::BlankLine);
}

#[test]
fn test_tokenize_prompt() {
    let regions = Tokenizer::tokenize("<<username\n<<role [admin, user]\n");
    assert_eq!(regions.len(), 2);
    match &regions[0] {
        Region::Prompt(rest) => assert_eq!(rest, "username"),
        _ => panic!("Expected Prompt"),
    }
}

#[test]
fn test_tokenize_prompt_commented() {
    let regions = Tokenizer::tokenize("# <<username\n# <<role [admin, user]\n");
    assert_eq!(regions.len(), 2);
    match &regions[0] {
        Region::Prompt(rest) => assert_eq!(rest, "username"),
        _ => panic!("Expected Prompt"),
    }
}

#[test]
fn test_tokenize_full_http_file() {
    let content = "import ./auth.http\n\n@base_url = https://api.example.com\n\n### Get users\n@page_size = 20\nGET {{base_url}}/users?limit={{page_size}}\nAccept: application/json\n\n{\n  \"name\": \"test\"\n}\n\n> {%\n  client.test(\"ok\", function() end)\n%}\n";
    let regions = Tokenizer::tokenize(content);
    assert!(regions.len() > 10);
}

// ─── Formatter tests ───

#[test]
fn test_format_var_spacing() {
    let input = "@base_url=https://api.example.com\n@token=abc123\n\n### Test\nGET /api\n";
    let output = Formatter::format(input);
    assert!(output.contains("@base_url = https://api.example.com"));
    assert!(output.contains("@token = abc123"));
}

#[test]
fn test_format_header_capitalization() {
    let input = "### Test\nGET /api\ncontent-type: application/json\n\n";
    let output = Formatter::format(input);
    assert!(output.contains("Content-Type: application/json"));
    assert!(!output.contains("content-type:"));
}

#[test]
fn test_format_separator_blank_line() {
    let input = "### First\nGET /api/1\n### Second\nGET /api/2\n";
    let output = Formatter::format(input);
    assert!(output.contains("### First\nGET /api/1\n\n### Second"));
}

#[test]
fn test_format_trailing_whitespace_removed() {
    let input = "### Test\nGET /api\ncontent-type: application/json    \n\n";
    let output = Formatter::format(input);
    assert!(!output.contains("    \n"));
}

#[test]
fn test_format_trailing_newline() {
    let input = "### Test\nGET /api\n";
    let output = Formatter::format(input);
    assert!(output.ends_with('\n'));
    let count = output.chars().filter(|&c| c == '\n').count();
    // Should have exactly: "### Test\nGET /api\n" = 2 newlines
    assert_eq!(count, 2);
}

#[test]
fn test_format_import_preserved() {
    let input = "import ./auth.http\n\n### Test\nGET /api\n";
    let output = Formatter::format(input);
    assert!(output.contains("import ./auth.http"));
}

#[test]
fn test_format_run_preserved() {
    let input = "import ./auth.http\n\n@base_url = x\n\n### Test\nGET /api\n\nrun #Login\n";
    let output = Formatter::format(input);
    assert!(output.contains("run #Login"));
}

#[test]
fn test_format_run_in_comment_block() {
    let input = "### 0.7a run #auth.Login — single imported request via alias\n# Place cursor on `run` line below, press <leader>r.\n# Expected: POST to httpbin.org/post → 200 + request JSON echoed back.\nrun #auth.Login\n";
    let output = Formatter::format(input);
    assert!(output.contains("run #auth.Login"));
}

#[test]
fn test_format_multiline_var_preserved() {
    let input = "@headers =>>>\nAuthorization: token\nX-Custom: yes\n<<<\n\n### Test\nGET /api\n{{headers}}\n";
    let output = Formatter::format(input);
    assert!(output.contains("@headers =>>>"));
    assert!(output.contains("Authorization: token"));
    assert!(output.contains("<<<"));
}

#[test]
fn test_format_prescript_preserved() {
    let input = "### Test\n< {%\n  local x = 1\n%}\nGET /api\n";
    let output = Formatter::format(input);
    assert!(output.contains("< {%"));
    assert!(output.contains("local x = 1"));
    assert!(output.contains("%}"));
}

#[test]
fn test_format_postscript_preserved() {
    let input = "### Test\nGET /api\n\n> {%\n  client.test(\"ok\", function() end)\n%}\n";
    let output = Formatter::format(input);
    assert!(output.contains("> {%"));
    assert!(output.contains("client.test"));
    assert!(output.contains("%}"));
}

#[test]
fn test_format_roundtrip_identity() {
    let input = "### Get users\nGET /api/users\nAccept: application/json\n\n{\"name\":\"test\"}\n";
    // Format should not lose information
    let output = Formatter::format(input);
    assert!(output.contains("GET /api/users"));
    assert!(output.contains("Accept: application/json"));
    // JSON body should be pretty-printed
    assert!(output.contains("\"name\": \"test\""));
    assert!(output.contains('{'));
    assert!(output.contains('}'));
}

#[test]
fn test_format_consecutive_blank_lines_compressed() {
    let input = "### First\nGET /api/1\n\n\n\n### Second\nGET /api/2\n";
    let output = Formatter::format(input);
    // Should have exactly one blank line between blocks
    assert!(output.contains("GET /api/1\n\n### Second"));
    assert!(!output.contains("GET /api/1\n\n\n### Second"));
}

#[test]
fn test_format_prompt_preserved() {
    let input = "<<username\n\n### Test\nGET /api\n";
    let output = Formatter::format(input);
    assert!(output.contains("<<username"));
}

#[test]
fn test_format_external_script_preserved() {
    let input = "### Test\n< ./scripts/gen.lua\nGET /api\n> ./scripts/check.lua\n";
    let output = Formatter::format(input);
    assert!(output.contains("< ./scripts/gen.lua"));
    assert!(output.contains("> ./scripts/check.lua"));
}

#[test]
fn test_reindent_empty() {
    let result = Formatter::reindent_code(&[]);
    assert!(result.is_empty());
}

#[test]
fn test_reindent_no_nesting() {
    let lines = vec![
        "    client.log(1)".to_string(),
        "  client.log(2)".to_string(),
    ];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result, vec!["client.log(1)", "client.log(2)"]);

    let output =
        Formatter::format("### Test\n< {%\n    client.log(1)\n  client.log(2)\n%}\nGET /api\n");
    assert!(output.contains("client.log(1)\nclient.log(2)"));
}

#[test]
fn test_reindent_function_body() {
    let lines = vec![
        "client.test(\"ok\", function()".to_string(),
        "client.assert(true)".to_string(),
        "end)".to_string(),
    ];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result[0], "client.test(\"ok\", function()");
    assert_eq!(result[1], "  client.assert(true)");
    assert_eq!(result[2], "end)");
}

#[test]
fn test_reindent_nested_functions() {
    let lines = vec![
        "fn_a(function()".to_string(),
        "fn_b(function()".to_string(),
        "inner()".to_string(),
        "end)".to_string(),
        "end)".to_string(),
    ];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result[0], "fn_a(function()");
    assert_eq!(result[1], "  fn_b(function()");
    assert_eq!(result[2], "    inner()");
    assert_eq!(result[3], "  end)");
    assert_eq!(result[4], "end)");
}

#[test]
fn test_reindent_if_then_end() {
    let lines = vec![
        "if x then".to_string(),
        "do_it()".to_string(),
        "end".to_string(),
    ];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result[0], "if x then");
    assert_eq!(result[1], "  do_it()");
    assert_eq!(result[2], "end");
}

#[test]
fn test_reindent_if_else_end() {
    let lines = vec![
        "if x then".to_string(),
        "a()".to_string(),
        "else".to_string(),
        "b()".to_string(),
        "end".to_string(),
    ];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result[0], "if x then");
    assert_eq!(result[1], "  a()");
    assert_eq!(result[2], "else");
    assert_eq!(result[3], "  b()");
    assert_eq!(result[4], "end");
}

#[test]
fn test_reindent_braces() {
    let lines = vec![
        "client.test(\"ok\", function() {".to_string(),
        "client.assert(true);".to_string(),
        "});".to_string(),
    ];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result[0], "client.test(\"ok\", function() {");
    assert_eq!(result[1], "  client.assert(true);");
    assert_eq!(result[2], "});");
}

#[test]
fn test_reindent_for_do_end() {
    let lines = vec![
        "for i=1,10 do".to_string(),
        "process(i)".to_string(),
        "end".to_string(),
    ];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result[0], "for i=1,10 do");
    assert_eq!(result[1], "  process(i)");
    assert_eq!(result[2], "end");
}

#[test]
fn test_reindent_blank_lines_preserved() {
    let lines = vec![
        "if x then".to_string(),
        "".to_string(),
        "do_it()".to_string(),
        "end".to_string(),
    ];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result[0], "if x then");
    assert_eq!(result[1], "");
    assert_eq!(result[2], "  do_it()");
    assert_eq!(result[3], "end");
}

#[test]
fn test_reindent_multiline_comment_no_false_positive() {
    let lines = vec!["do_it()".to_string(), "-- this is not an end".to_string()];
    let result = Formatter::reindent_code(&lines);
    assert_eq!(result[0], "do_it()");
    assert_eq!(result[1], "-- this is not an end");
}

#[test]
fn test_format_prescript_reindented() {
    let input = "### Test\n< {%\n  client.test(\"ok\", function()\n    client.assert(true)\n  end)\n%}\nGET /api\n";
    let output = Formatter::format(input);
    let expected = "### Test\n< {%\nclient.test(\"ok\", function()\n  client.assert(true)\nend)\n%}\nGET /api\n";
    assert_eq!(output, expected);
}

#[test]
fn test_format_prescript_fixes_bad_indent() {
    let input = "### Test\n< {%\n        client.test(\"ok\", function()\n                client.assert(true)\n        end)\n%}\nGET /api\n";
    let output = Formatter::format(input);
    let expected = "### Test\n< {%\nclient.test(\"ok\", function()\n  client.assert(true)\nend)\n%}\nGET /api\n";
    assert_eq!(output, expected);
}

// ─── JSON body formatting tests ───

#[test]
fn test_format_json_body_prettyprints() {
    let input = "### Test\nPOST /api/data\nContent-Type: application/json\n\n{\"name\":\"test\",\"value\":123}\n";
    let output = Formatter::format(input);
    assert!(output.contains("\"name\": \"test\""));
    assert!(output.contains("\"value\": 123"));
    assert!(output.starts_with("### Test\nPOST /api/data\nContent-Type: application/json\n\n{"));
    // Closing brace should align with opening brace
    assert!(output.contains("\n}\n"));
}

#[test]
fn test_format_json_body_preserves_pretty() {
    let input = "### Test\nPOST /api/data\nContent-Type: application/json\n\n{\n  \"name\": \"test\",\n  \"value\": 123\n}\n";
    let output = Formatter::format(input);
    assert!(output.contains("\"name\": \"test\""));
    assert!(output.contains("\"value\": 123"));
}

#[test]
fn test_format_json_body_with_variables() {
    let input = "### Test\nPOST /api/login\nContent-Type: application/json\n\n{\n  \"username\": \"{{username}}\",\n  \"password\": \"{{password}}\"\n}\n";
    let output = Formatter::format(input);
    // Variables inside JSON strings should be preserved
    assert!(output.contains("{{username}}"));
    assert!(output.contains("{{password}}"));
}

#[test]
fn test_format_json_body_debug_raw() {
    // Debug: verify the raw JSON is parseable
    let input = "### Test\nPOST /api/data\nContent-Type: application/json\n\n{\"name\":\"doge\",\n    \"value\":13\n\n}\n\n# ─────────────────────\n# 0.1 {{$magic}}\n";
    let output = Formatter::format(input);
    eprintln!("OUTPUT: {:?}", output);
    assert!(output.contains("\"name\": \"doge\"") || output.contains("\"name\": \"doge\","));
    assert!(output.contains("\"value\": 13") || output.contains("\"value\": 13,"));
    // Trailing comments should be preserved
    assert!(output.contains("# ─────────────────────"));
    assert!(output.contains("# 0.1 {{$magic}}"));
}

#[test]
fn test_format_json_body_strip_comments() {
    // Direct test of strip_trailing_body_lines
    let body = vec![
        "{\"name\":\"doge\",".to_string(),
        "    \"value\":13".to_string(),
        "".to_string(),
        "}".to_string(),
        "".to_string(),
        "# ─────────────────────".to_string(),
        "# 0.1 {{$magic}}".to_string(),
    ];
    let end = Formatter::find_json_body_end(&body);
    assert_eq!(end, 4);
    let json_part = &body[..end];
    let joined = json_part.join("\n");
    let result = serde_json::from_str::<serde_json::Value>(&joined);
    assert!(
        result.is_ok(),
        "JSON should be valid, got: {:?}",
        result.err()
    );
}

#[test]
fn test_format_json_body_with_trailing_comments() {
    let input = "### Test\nPOST /api/data\nContent-Type: application/json\n\n{\"name\":\"doge\",\n    \"value\":13\n\n}\n\n# ─────────────────────\n# 0.1 {{$magic}}\n";
    let output = Formatter::format(input);
    assert!(output.contains("\"name\": \"doge\""));
    assert!(output.contains("\"value\": 13"));
    assert!(output.contains("# ─────────────────────"));
    assert!(output.contains("# 0.1 {{$magic}}"));
}

#[test]
fn test_format_non_json_body_unchanged() {
    let input = "### Test\nPOST /api/data\nContent-Type: application/x-www-form-urlencoded\n\nkey1=value1&key2=value2\n";
    let output = Formatter::format(input);
    assert!(output.contains("key1=value1&key2=value2"));
}

#[test]
fn test_format_empty_body_unchanged() {
    let input = "### Test\nGET /api\n";
    let output = Formatter::format(input);
    assert_eq!(output, "### Test\nGET /api\n");
}

#[test]
fn test_format_json_array_body() {
    let input =
        "### Test\nPOST /api/items\nContent-Type: application/json\n\n[{\"id\":1},{\"id\":2}]\n";
    let output = Formatter::format(input);
    assert!(output.contains("\"id\": 1"));
    assert!(output.contains("\"id\": 2"));
}
