use crate::import::{HttpFile, ImportResult, SpecImporter};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;

/// Import from Postman Collection v2.1 export.
pub struct PostmanImporter;

impl SpecImporter for PostmanImporter {
    fn import(&self, spec_content: &str) -> Result<ImportResult> {
        let collection: Value = serde_json::from_str(spec_content)
            .context("Failed to parse Postman collection (JSON expected)")?;

        // Verify it's a Postman collection
        let schema = collection
            .get("info")
            .and_then(|i| i.get("schema"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if !schema.contains("postman") {
            anyhow::bail!("Not a Postman collection (schema field missing or invalid)");
        }

        let mut files = Vec::new();
        let mut env_vars = HashMap::new();
        let mut warnings = Vec::new();

        // Extract collection-level variables
        if let Some(vars) = collection.get("variable").and_then(|v| v.as_array()) {
            for var in vars {
                if let Some(key) = var.get("key").and_then(|k| k.as_str()) {
                    let value = var
                        .get("value")
                        .map(|v| {
                            if let Some(s) = v.as_str() {
                                s.to_string()
                            } else {
                                serde_json::to_string(v).unwrap_or_default()
                            }
                        })
                        .unwrap_or_default();
                    env_vars.entry(key.to_string()).or_insert(value);
                }
            }
        }

        // Extract base_url from the first request's URL or collection vars
        let base_url = env_vars
            .get("base_url")
            .cloned()
            .or_else(|| env_vars.get("baseUrl").cloned())
            .unwrap_or_else(|| "http://localhost".to_string());
        if !env_vars.contains_key("base_url") {
            env_vars.insert("base_url".to_string(), base_url.clone());
        }

        // Process items recursively
        if let Some(items) = collection.get("item").and_then(|v| v.as_array()) {
            if items.is_empty() {
                return Ok(ImportResult {
                    files: vec![],
                    env_vars,
                    warnings,
                });
            }

            let mut content = String::new();
            content.push_str("@base_url = {{base_url}}\n");
            content.push('\n');

            process_items(
                items,
                &mut content,
                &mut env_vars,
                &mut warnings,
                &base_url,
                "",
            );

            if !content.trim().is_empty() {
                // Determine filename from collection name
                let name = collection
                    .get("info")
                    .and_then(|i| i.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("collection");
                let filename = format!("{}.http", sanitize_filename(name));
                files.push(HttpFile {
                    path: filename,
                    content,
                });
            }
        }

        Ok(ImportResult {
            files,
            env_vars,
            warnings,
        })
    }
}

/// Process a list of Postman items (requests and folders).
fn process_items(
    items: &[Value],
    content: &mut String,
    env_vars: &mut HashMap<String, String>,
    _warnings: &mut Vec<String>,
    base_url: &str,
    _folder_path: &str,
) {
    for item in items {
        // Check if it's a folder (has its own "item" array)
        if let Some(sub_items) = item.get("item").and_then(|v| v.as_array()) {
            // It's a folder — process items inline
            let folder_name = item
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("folder");
            content.push_str(&format!("# --- {} ---\n", folder_name));
            process_items(
                sub_items,
                content,
                env_vars,
                _warnings,
                base_url,
                folder_name,
            );
            content.push('\n');
            continue;
        }

        // It's a request
        let name = item
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Unnamed");
        let request = match item.get("request") {
            Some(r) => r,
            None => continue,
        };

        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        // Parse URL
        let url_str = extract_url(request, &method, base_url, env_vars);

        // Pre-request / test scripts from item.event[]
        if let Some(events) = item.get("event").and_then(|e| e.as_array()) {
            for event in events {
                let listen = event.get("listen").and_then(|l| l.as_str()).unwrap_or("");
                let script_lines = event
                    .get("script")
                    .and_then(|s| s.get("exec"))
                    .and_then(|e| e.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>());
                if let Some(lines) = script_lines {
                    if !lines.is_empty() {
                        if listen == "prerequest" {
                            content.push_str("< {%\n");
                            for line in &lines {
                                content.push_str(&format!("  {}\n", line));
                            }
                            content.push_str("%}\n");
                        } else if listen == "test" {
                            content.push_str("> {%\n");
                            for line in &lines {
                                content.push_str(&format!("  {}\n", line));
                            }
                            content.push_str("%}\n");
                        }
                    }
                }
            }
        }

        // Write request block
        content.push_str(&format!("### {}\n", name));
        content.push_str(&format!("{} {}\n", method, url_str));

        // Headers
        if let Some(headers) = request.get("header").and_then(|h| h.as_array()) {
            for header in headers {
                let key = header.get("key").and_then(|k| k.as_str()).unwrap_or("");
                let value = header.get("value").and_then(|v| v.as_str()).unwrap_or("");
                if !key.is_empty() {
                    content.push_str(&format!("{}: {}\n", key, value));
                }
            }
        }

        // Body
        if let Some(body) = request.get("body") {
            let mode = body.get("mode").and_then(|m| m.as_str()).unwrap_or("");
            match mode {
                "raw" => {
                    if let Some(raw) = body.get("raw").and_then(|r| r.as_str()) {
                        // Detect content type from options or raw content
                        let content_type = body
                            .get("options")
                            .and_then(|o| o.get("raw"))
                            .and_then(|r| r.get("language"))
                            .and_then(|l| l.as_str())
                            .map(|lang| match lang {
                                "json" => "application/json",
                                "xml" => "application/xml",
                                "text" => "text/plain",
                                "html" => "text/html",
                                _ => "application/json",
                            })
                            .unwrap_or("application/json");
                        content.push_str(&format!("Content-Type: {}\n", content_type));
                        content.push('\n');
                        content.push_str(raw);
                        content.push('\n');
                    }
                }
                "urlencoded" => {
                    content.push_str("Content-Type: application/x-www-form-urlencoded\n");
                    content.push('\n');
                    if let Some(params) = body.get("urlencoded").and_then(|u| u.as_array()) {
                        let parts: Vec<String> = params
                            .iter()
                            .filter_map(|p| {
                                let k = p.get("key").and_then(|k| k.as_str())?;
                                let v = p.get("value").and_then(|v| v.as_str()).unwrap_or("");
                                Some(format!("{}={}", k, v))
                            })
                            .collect();
                        if !parts.is_empty() {
                            content.push_str(&parts.join("&"));
                            content.push('\n');
                        }
                    }
                }
                "formdata" => {
                    content.push_str("Content-Type: multipart/form-data\n");
                    content.push('\n');
                    if let Some(params) = body.get("formdata").and_then(|f| f.as_array()) {
                        for param in params {
                            let key = param.get("key").and_then(|k| k.as_str()).unwrap_or("field");
                            let value = param.get("value").and_then(|v| v.as_str()).unwrap_or("");
                            if param.get("src").is_some() {
                                content.push_str(&format!("< ./path/to/{}\n", key));
                            } else {
                                content.push_str(&format!("{}: {}\n", key, value));
                            }
                        }
                    }
                }
                "file" => {
                    content.push_str("# <request body from file>\n");
                }
                _ => {}
            }
        }

        content.push('\n');
    }
}

/// Extract URL from a Postman request object.
fn extract_url(
    request: &Value,
    _method: &str,
    _base_url: &str,
    env_vars: &mut HashMap<String, String>,
) -> String {
    let url = match request.get("url") {
        Some(u) => u,
        None => return "{{base_url}}".to_string(),
    };

    // If URL is a raw string, use it directly
    if let Some(raw) = url.get("raw").and_then(|r| r.as_str()) {
        // Replace Postman variable syntax {{var}} with Poste {{var}} (same syntax!)
        let processed = replace_postman_vars(raw, env_vars);
        // If it doesn't start with base_url, resolve it
        if processed.contains("://") {
            // Absolute URL — extract base for env vars
            if let Some(pos) = processed.rfind("://") {
                let after_proto = &processed[pos + 3..];
                if let Some(slash_pos) = after_proto.find('/') {
                    let extracted_base = &processed[..pos + 3 + slash_pos];
                    env_vars
                        .entry("base_url".to_string())
                        .or_insert_with(|| extracted_base.to_string());
                    // Return relative path with {{base_url}} prefix
                    let path = &after_proto[slash_pos..];
                    return format!("{{{{base_url}}}}{}", path);
                }
            }
            return processed;
        } else if processed.starts_with('/') {
            return format!("{{{{base_url}}}}{}", processed);
        }
        return processed;
    }

    // Structured URL object
    let host_parts: Vec<String> = url
        .get("host")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    let path_parts: Vec<String> = url
        .get("path")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    if host_parts.is_empty() && path_parts.is_empty() {
        return "{{base_url}}".to_string();
    }

    let protocol = url
        .get("protocol")
        .and_then(|p| p.as_str())
        .unwrap_or("https");
    let host = host_parts.join(".");
    let port = url.get("port").and_then(|p| p.as_str());

    let full_host = match port {
        Some(p) => format!("{}://{}:{}", protocol, host, p),
        None => format!("{}://{}", protocol, host),
    };

    let path = if path_parts.is_empty() {
        String::new()
    } else {
        format!("/{}", replace_postman_vars(&path_parts.join("/"), env_vars))
    };

    // Query params
    let query_str = if let Some(query) = url.get("query").and_then(|q| q.as_array()) {
        let parts: Vec<String> = query
            .iter()
            .filter_map(|p| {
                let k = p.get("key").and_then(|k| k.as_str())?;
                let v = p.get("value").and_then(|v| v.as_str()).unwrap_or("");
                Some(format!("{}={}", k, replace_postman_vars(v, env_vars)))
            })
            .collect();
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    } else {
        String::new()
    };

    // If the full host doesn't match base_url, set it as base_url
    let url_string = format!("{}{}{}", full_host, path, query_str);
    env_vars.entry("base_url".to_string()).or_insert_with(|| {
        if host_parts.len() > 1 {
            format!(
                "{}://{}{}",
                protocol,
                host,
                if let Some(p) = port {
                    format!(":{}", p)
                } else {
                    String::new()
                }
            )
        } else {
            full_host.clone()
        }
    });

    // Return relative URL with {{base_url}}
    if !path.is_empty() {
        format!("{{{{base_url}}}}{}{}", path, query_str)
    } else {
        url_string
    }
}

/// Replace Postman variable references (same {{var}} syntax as Poste).
fn replace_postman_vars(s: &str, env_vars: &mut HashMap<String, String>) -> String {
    // Postman and Poste use the same {{var}} syntax, so we keep them as-is
    // But also extract any unknown vars into env_vars
    let re = regex::Regex::new(r"\{\{([^}]+)\}\}").unwrap();
    for cap in re.captures_iter(s) {
        let var_name = cap[1].to_string();
        env_vars.entry(var_name).or_default();
    }
    s.to_string()
}

/// Sanitize a string for use as a filename.
fn sanitize_filename(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "collection".to_string()
    } else {
        s.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_collection() {
        let spec = r#"{
            "info": {
                "name": "Empty",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": []
        }"#;
        let importer = PostmanImporter;
        let result = importer.import(spec).unwrap();
        assert!(result.files.is_empty());
    }

    #[test]
    fn test_basic_get_request() {
        let spec = r#"{
            "info": {
                "name": "Test API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [{
                "name": "List Items",
                "request": {
                    "method": "GET",
                    "url": {
                        "raw": "https://api.example.com/v1/items",
                        "host": ["api", "example", "com"],
                        "path": ["v1", "items"]
                    }
                }
            }]
        }"#;
        let importer = PostmanImporter;
        let result = importer.import(spec).unwrap();
        assert_eq!(result.files.len(), 1);
        let c = &result.files[0].content;
        assert!(c.contains("### List Items"), "request name: {}", c);
        assert!(
            c.contains("GET {{base_url}}/v1/items"),
            "request line: {}",
            c
        );
    }

    #[test]
    fn test_post_request_with_body() {
        let spec = r#"{
            "info": {
                "name": "Test API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [{
                "name": "Create Item",
                "request": {
                    "method": "POST",
                    "header": [{
                        "key": "X-Custom",
                        "value": "test123"
                    }],
                    "body": {
                        "mode": "raw",
                        "raw": "{\"name\": \"New Item\"}",
                        "options": { "raw": { "language": "json" } }
                    },
                    "url": {
                        "raw": "https://api.example.com/v1/items",
                        "host": ["api", "example", "com"],
                        "path": ["v1", "items"]
                    }
                }
            }]
        }"#;
        let importer = PostmanImporter;
        let result = importer.import(spec).unwrap();
        let c = &result.files[0].content;
        assert!(c.contains("### Create Item"), "request name: {}", c);
        assert!(
            c.contains("POST {{base_url}}/v1/items"),
            "request line: {}",
            c
        );
        assert!(c.contains("X-Custom: test123"), "header: {}", c);
        assert!(
            c.contains("Content-Type: application/json"),
            "content type: {}",
            c
        );
        assert!(c.contains(r#"{"name": "New Item"}"#), "body: {}", c);
    }

    #[test]
    fn test_nested_folders() {
        let spec = r#"{
            "info": {
                "name": "Nested API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [{
                "name": "Users",
                "item": [{
                    "name": "Get User",
                    "request": {
                        "method": "GET",
                        "url": {
                            "raw": "https://api.example.com/users/1",
                            "host": ["api", "example", "com"],
                            "path": ["users", "1"]
                        }
                    }
                }]
            }]
        }"#;
        let importer = PostmanImporter;
        let result = importer.import(spec).unwrap();
        assert_eq!(result.files.len(), 1);
        let c = &result.files[0].content;
        assert!(c.contains("### Get User"), "request name: {}", c);
        assert!(c.contains("# --- Users ---"), "folder comment: {}", c);
    }

    #[test]
    fn test_collection_variables_to_env() {
        let spec = r#"{
            "info": {
                "name": "Config API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "variable": [
                { "key": "base_url", "value": "https://api.example.com" },
                { "key": "api_key", "value": "" }
            ],
            "item": [{
                "name": "Get Data",
                "request": {
                    "method": "GET",
                    "url": {
                        "raw": "{{base_url}}/data",
                        "host": ["api", "example", "com"],
                        "path": ["data"]
                    }
                }
            }]
        }"#;
        let importer = PostmanImporter;
        let result = importer.import(spec).unwrap();
        assert!(result.env_vars.contains_key("base_url"));
        assert!(result.env_vars.contains_key("api_key"));
        assert_eq!(
            result.env_vars.get("base_url").unwrap(),
            "https://api.example.com"
        );
    }

    #[test]
    fn test_urlencoded_body() {
        let spec = r#"{
            "info": {
                "name": "Form API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [{
                "name": "Submit Form",
                "request": {
                    "method": "POST",
                    "body": {
                        "mode": "urlencoded",
                        "urlencoded": [
                            { "key": "name", "value": "john" },
                            { "key": "age", "value": "30" }
                        ]
                    },
                    "url": {
                        "raw": "https://api.example.com/form",
                        "host": ["api", "example", "com"],
                        "path": ["form"]
                    }
                }
            }]
        }"#;
        let importer = PostmanImporter;
        let result = importer.import(spec).unwrap();
        let c = &result.files[0].content;
        assert!(
            c.contains("Content-Type: application/x-www-form-urlencoded"),
            "content type: {}",
            c
        );
        assert!(c.contains("name=john"), "form field: {}", c);
        assert!(c.contains("age=30"), "form field: {}", c);
    }

    #[test]
    fn test_query_parameters_from_url() {
        let spec = r#"{
            "info": {
                "name": "Query API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [{
                "name": "Search",
                "request": {
                    "method": "GET",
                    "url": {
                        "raw": "https://api.example.com/search?q=hello&page=1",
                        "host": ["api", "example", "com"],
                        "path": ["search"],
                        "query": [
                            { "key": "q", "value": "hello" },
                            { "key": "page", "value": "1" }
                        ]
                    }
                }
            }]
        }"#;
        let importer = PostmanImporter;
        let result = importer.import(spec).unwrap();
        let c = &result.files[0].content;
        assert!(c.contains("?q=hello"), "query param: {}", c);
        assert!(c.contains("page=1"), "query param: {}", c);
    }

    // -----------------------------------------------------------------------
    // Step 12: Pre-request and test scripts
    // -----------------------------------------------------------------------

    #[test]
    fn test_prerequest_and_test_scripts() {
        let spec = r#"{
            "info": {
                "name": "Scripted API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [{
                "name": "Get Data",
                "event": [
                    {
                        "listen": "prerequest",
                        "script": {
                            "exec": ["pm.variables.set(\"token\", \"abc\")"],
                            "type": "text/javascript"
                        }
                    },
                    {
                        "listen": "test",
                        "script": {
                            "exec": ["pm.response.to.have.status(200)"],
                            "type": "text/javascript"
                        }
                    }
                ],
                "request": {
                    "method": "GET",
                    "url": {
                        "raw": "https://api.example.com/data",
                        "host": ["api", "example", "com"],
                        "path": ["data"]
                    }
                }
            }]
        }"#;
        let importer = PostmanImporter;
        let result = importer.import(spec).unwrap();
        let c = &result.files[0].content;
        assert!(c.contains("< {%"), "should have pre-script block: {}", c);
        assert!(c.contains("pm.variables.set"), "prescript content: {}", c);
        assert!(c.contains("> {%"), "should have post-script block: {}", c);
        assert!(
            c.contains("pm.response.to.have.status"),
            "postscript content: {}",
            c
        );
    }
}
