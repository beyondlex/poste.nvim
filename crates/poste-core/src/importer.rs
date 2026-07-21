use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ImportDirective {
    Bare { path: String },
    Aliased { path: String, alias: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunDirective {
    ByName {
        name: String,
        vars: HashMap<String, String>,
    },
    ByAlias {
        alias: String,
        name: String,
        vars: HashMap<String, String>,
    },
    ByPath {
        path: String,
        vars: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ImportIndex {
    pub bare_imports: Vec<ImportDirective>,
    pub aliased_imports: HashMap<String, ImportDirective>,
}

/// Parse a line into an ImportDirective if it matches `import <path>` or
/// `import <path> as <alias>`.
pub fn parse_import(line: &str) -> Option<ImportDirective> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("import ")?;

    // Try: import ./path as alias
    if let Some((path, alias)) = rest.split_once(" as ") {
        let path = path.trim().to_string();
        let alias = alias.trim().to_string();
        if !path.is_empty()
            && !alias.is_empty()
            && alias.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            return Some(ImportDirective::Aliased { path, alias });
        }
    }

    // Try: import ./path
    let path = rest.trim().to_string();
    if !path.is_empty() {
        Some(ImportDirective::Bare { path })
    } else {
        None
    }
}

/// Parse a line into a RunDirective if it matches `run ...`.
pub fn parse_run(line: &str) -> Option<RunDirective> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("run ")?;

    // Extract optional inline variables: run #Name (@key=val)
    let (target, vars_str) = if let Some(idx) = rest.find(" (") {
        let end = rest.rfind(')')?;
        let vars_part = &rest[idx + 2..end];
        let target = rest[..idx].trim().to_string();
        (target, Some(vars_part.to_string()))
    } else {
        (rest.trim().to_string(), None)
    };

    let vars = parse_run_vars(vars_str.as_deref());

    // run #alias.Name
    if let Some(after_hash) = target.strip_prefix('#') {
        if let Some(dot_pos) = after_hash.find('.') {
            let alias = after_hash[..dot_pos].to_string();
            let name = after_hash[dot_pos + 1..].to_string();
            if !alias.is_empty() && !name.is_empty() {
                return Some(RunDirective::ByAlias { alias, name, vars });
            }
        }
    }

    // run #Name
    if let Some(name) = target.strip_prefix('#') {
        if !name.is_empty() {
            return Some(RunDirective::ByName {
                name: name.to_string(),
                vars,
            });
        }
    }

    // run ./path
    if !target.is_empty() {
        Some(RunDirective::ByPath { path: target, vars })
    } else {
        None
    }
}

fn parse_run_vars(vars_str: Option<&str>) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    if let Some(s) = vars_str {
        for pair in s.split(',') {
            let pair = pair.trim();
            if let Some((key, value)) = pair.split_once('=') {
                let key = key.trim().trim_start_matches('@').to_string();
                let value = value.trim().to_string();
                if !key.is_empty() {
                    vars.insert(key, value);
                }
            }
        }
    }
    vars
}

/// Parse all import directives from file content (before first `###` block).
/// Returns a list of import directives in order of appearance.
pub fn parse_imports_from_content(content: &str) -> Vec<ImportDirective> {
    let mut directives = Vec::new();
    for line in content.lines() {
        if line.trim().starts_with("###") {
            break;
        }
        if let Some(d) = parse_import(line) {
            directives.push(d);
        }
    }
    directives
}

/// Parse all run directives from file content (file-level and between blocks).
/// Returns a list of run directives in order of appearance.
pub fn parse_runs_from_content(content: &str) -> Vec<RunDirective> {
    let mut directives = Vec::new();
    for line in content.lines() {
        if let Some(d) = parse_run(line) {
            directives.push(d);
        }
    }
    directives
}

/// Build an ImportIndex from a list of import directives.
/// Returns the index and any errors (conflicting aliases).
pub fn build_index(imports: &[ImportDirective]) -> (ImportIndex, Vec<String>) {
    let mut index = ImportIndex::default();
    let mut errors = Vec::new();
    let mut used_aliases = HashMap::new();

    for directive in imports {
        match directive {
            ImportDirective::Bare { .. } => {
                index.bare_imports.push(directive.clone());
            }
            ImportDirective::Aliased { alias, .. } => {
                if used_aliases.contains_key(alias) {
                    errors.push(format!("Duplicate alias '{}'", alias));
                } else {
                    used_aliases.insert(alias.clone(), true);
                    index
                        .aliased_imports
                        .insert(alias.clone(), directive.clone());
                }
            }
        }
    }

    (index, errors)
}

/// Extract all named request blocks from file content.
/// Returns (1-indexed line_number, name) for each `### Name` block.
pub fn extract_request_names(content: &str) -> Vec<(usize, String)> {
    let mut names = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("###") {
            let name = name.trim().to_string();
            if !name.is_empty() {
                names.push((i + 1, name));
            }
        }
    }
    names
}

/// Find the content of a named request block (`### Name ...` until next `###` or EOF).
/// Returns the block content (including the `###` line) and its starting line (1-indexed).
pub fn find_block_by_name(content: &str, name: &str) -> Option<(usize, String)> {
    let pattern = format!("### {}", name);
    let mut block_lines: Vec<&str> = Vec::new();
    let mut capture = false;
    let mut start_line = 0;

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("###") {
            if capture {
                // End of the block we were capturing
                break;
            }
            if trimmed == pattern || trimmed == format!("###{}", name) {
                capture = true;
                start_line = i + 1;
                block_lines.push(line);
            }
        } else if capture {
            block_lines.push(line);
        }
    }

    if capture {
        Some((start_line, block_lines.join("\n")))
    } else {
        None
    }
}

/// Check if a line is an import or run directive.
pub fn is_import_or_run_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("import ") || trimmed.starts_with("run ")
}

/// Check for name collisions among bare imports.
/// Returns a list of warning messages.
pub fn check_bare_collisions(requests_by_file: &[(&str, Vec<&str>)]) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut seen: HashMap<&str, &str> = HashMap::new();

    for (file, names) in requests_by_file {
        for name in names {
            if let Some(prev_file) = seen.get(name) {
                warnings.push(format!(
                    "Warning: request '{}' in {} overrides same name in {}",
                    name, file, prev_file
                ));
            }
            seen.insert(name, file);
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_import_bare() {
        let d = parse_import("import ./auth.http").unwrap();
        assert_eq!(
            d,
            ImportDirective::Bare {
                path: "./auth.http".to_string()
            }
        );
    }

    #[test]
    fn test_parse_import_aliased() {
        let d = parse_import("import ./orders.http as orders").unwrap();
        assert_eq!(
            d,
            ImportDirective::Aliased {
                path: "./orders.http".to_string(),
                alias: "orders".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_import_not_match() {
        assert!(parse_import("### Request").is_none());
        assert!(parse_import("@var = value").is_none());
        assert!(parse_import("").is_none());
    }

    #[test]
    fn test_parse_run_by_name() {
        let d = parse_run("run #Login").unwrap();
        assert_eq!(
            d,
            RunDirective::ByName {
                name: "Login".to_string(),
                vars: HashMap::new(),
            }
        );
    }

    #[test]
    fn test_parse_run_by_alias() {
        let d = parse_run("run #orders.ListOrders").unwrap();
        assert_eq!(
            d,
            RunDirective::ByAlias {
                alias: "orders".to_string(),
                name: "ListOrders".to_string(),
                vars: HashMap::new(),
            }
        );
    }

    #[test]
    fn test_parse_run_with_vars() {
        let d = parse_run("run #Login (@token=xyz)").unwrap();
        let mut expected_vars = HashMap::new();
        expected_vars.insert("token".to_string(), "xyz".to_string());
        assert_eq!(
            d,
            RunDirective::ByName {
                name: "Login".to_string(),
                vars: expected_vars,
            }
        );
    }

    #[test]
    fn test_parse_run_with_multi_vars() {
        let d = parse_run("run #Login (@token=xyz, @env=staging)").unwrap();
        let mut expected_vars = HashMap::new();
        expected_vars.insert("token".to_string(), "xyz".to_string());
        expected_vars.insert("env".to_string(), "staging".to_string());
        assert_eq!(
            d,
            RunDirective::ByName {
                name: "Login".to_string(),
                vars: expected_vars,
            }
        );
    }

    #[test]
    fn test_parse_run_by_path() {
        let d = parse_run("run ./batch.http").unwrap();
        assert_eq!(
            d,
            RunDirective::ByPath {
                path: "./batch.http".to_string(),
                vars: HashMap::new(),
            }
        );
    }

    #[test]
    fn test_parse_imports_from_content() {
        let content =
            "import ./auth.http\nimport ./orders.http as orders\n\n### Request\nGET /api\n";
        let imports = parse_imports_from_content(content);
        assert_eq!(imports.len(), 2);
        assert_eq!(
            imports[0],
            ImportDirective::Bare {
                path: "./auth.http".to_string()
            }
        );
    }

    #[test]
    fn test_parse_runs_from_content() {
        let content = "### Get users\nGET /api\n\nrun #Login\n\nrun #orders.ListOrders\n";
        let runs = parse_runs_from_content(content);
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn test_build_index_no_conflicts() {
        let imports = vec![
            ImportDirective::Bare {
                path: "./a.http".to_string(),
            },
            ImportDirective::Aliased {
                path: "./b.http".to_string(),
                alias: "b".to_string(),
            },
        ];
        let (index, errors) = build_index(&imports);
        assert!(errors.is_empty());
        assert_eq!(index.bare_imports.len(), 1);
        assert_eq!(index.aliased_imports.len(), 1);
    }

    #[test]
    fn test_build_index_alias_conflict() {
        let imports = vec![
            ImportDirective::Aliased {
                path: "./a.http".to_string(),
                alias: "ns".to_string(),
            },
            ImportDirective::Aliased {
                path: "./b.http".to_string(),
                alias: "ns".to_string(),
            },
        ];
        let (_index, errors) = build_index(&imports);
        assert!(!errors.is_empty());
        assert!(errors[0].contains("Duplicate"));
    }

    #[test]
    fn test_check_bare_collisions() {
        let requests = vec![
            ("./a.http", vec!["Login", "Logout"]),
            ("./b.http", vec!["Login", "Profile"]),
        ];
        let warnings = check_bare_collisions(&requests);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Login"));
    }

    #[test]
    fn test_run_variable_override_parsing() {
        let d = parse_run("run #Login (@page_size=50, @format=json)").unwrap();
        if let RunDirective::ByName { vars, .. } = d {
            assert_eq!(vars.get("page_size"), Some(&"50".to_string()));
            assert_eq!(vars.get("format"), Some(&"json".to_string()));
        } else {
            panic!("Expected ByName");
        }
    }

    #[test]
    fn test_import_alias_with_underscore() {
        let d = parse_import("import ./my_orders.http as my_orders").unwrap();
        assert_eq!(
            d,
            ImportDirective::Aliased {
                path: "./my_orders.http".to_string(),
                alias: "my_orders".to_string(),
            }
        );
    }

    // ---- extract_request_names ----

    #[test]
    fn test_extract_request_names_basic() {
        let content =
            "import ./auth.http\n\n### Login\nGET /api/login\n\n### Logout\nGET /api/logout\n";
        let names = extract_request_names(content);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], (3, "Login".to_string()));
        assert_eq!(names[1], (6, "Logout".to_string()));
    }

    #[test]
    fn test_extract_request_names_empty() {
        let content = "import ./auth.http\n\n@var = value\n";
        let names = extract_request_names(content);
        assert!(names.is_empty());
    }

    #[test]
    fn test_extract_request_names_no_name() {
        let content = "###\nGET /api\n";
        let names = extract_request_names(content);
        assert!(names.is_empty());
    }

    // ---- find_block_by_name ----

    #[test]
    fn test_find_block_by_name_basic() {
        let content = "### Login\nGET /api/login\n\n### Logout\nGET /api/logout\n";
        let (line, block) = find_block_by_name(content, "Login").unwrap();
        assert_eq!(line, 1);
        assert!(block.contains("GET /api/login"));
        assert!(!block.contains("Logout"));
    }

    #[test]
    fn test_find_block_by_name_not_found() {
        let content = "### Login\nGET /api/login\n";
        assert!(find_block_by_name(content, "Missing").is_none());
    }

    #[test]
    fn test_find_block_by_name_second_block() {
        let content = "### Login\nGET /api/login\n\n### Logout\nGET /api/logout\n";
        let (line, block) = find_block_by_name(content, "Logout").unwrap();
        assert_eq!(line, 4);
        assert!(block.contains("GET /api/logout"));
    }

    #[test]
    fn test_find_block_by_name_without_space() {
        let content = "###Login\nGET /api/login\n";
        let (line, block) = find_block_by_name(content, "Login").unwrap();
        assert_eq!(line, 1);
        assert!(block.contains("GET /api/login"));
    }

    // ---- is_import_or_run_line ----

    #[test]
    fn test_is_import_line() {
        assert!(is_import_or_run_line("import ./auth.http"));
        assert!(is_import_or_run_line("import ./orders.http as orders"));
        assert!(is_import_or_run_line("  import ./auth.http"));
    }

    #[test]
    fn test_is_run_line() {
        assert!(is_import_or_run_line("run #Login"));
        assert!(is_import_or_run_line("run #orders.ListOrders"));
        assert!(is_import_or_run_line("run ./batch.http"));
        assert!(is_import_or_run_line("  run #Login (@token=xyz)"));
    }

    #[test]
    fn test_is_not_import_or_run() {
        assert!(!is_import_or_run_line("### Login"));
        assert!(!is_import_or_run_line("@var = value"));
        assert!(!is_import_or_run_line("GET /api"));
        assert!(!is_import_or_run_line(""));
    }

    #[test]
    fn test_extract_request_names_with_imports() {
        let content = "import ./auth.http\nimport ./orders.http as orders\n\n### Login\nGET /api/login\n\n### Logout\nGET /api/logout\n";
        let names = extract_request_names(content);
        assert_eq!(names.len(), 2);
        // Import lines before first ### should not produce request names
        assert_eq!(names[0].1, "Login");
        assert_eq!(names[1].1, "Logout");
    }
}
