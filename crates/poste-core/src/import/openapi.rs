use crate::import::{HttpFile, ImportResult, SpecImporter};
use anyhow::{Context, Result};
use openapiv3::{
    ObjectType, OpenAPI, Operation, Parameter, ParameterSchemaOrContent, PathItem, ReferenceOr,
    RequestBody, Schema, SchemaKind, Type,
};
use std::collections::HashMap;

/// Import from OpenAPI 3.x spec.
pub struct OpenApiImporter {
    /// Default base URL override (overrides spec's servers[0])
    pub base_url: Option<String>,
}

impl OpenApiImporter {
    pub fn new() -> Self {
        Self { base_url: None }
    }

    pub fn with_base_url(url: &str) -> Self {
        Self {
            base_url: Some(url.to_string()),
        }
    }

    /// Extract the base URL from the spec or use override.
    fn resolve_base_url(&self, api: &OpenAPI) -> String {
        if let Some(ref url) = self.base_url {
            return url.clone();
        }
        if let Some(server) = api.servers.first() {
            let url = server.url.trim_end_matches('/').to_string();
            if !url.is_empty() {
                return url;
            }
        }
        "http://localhost".to_string()
    }

    /// Collect all operations grouped by tag.
    /// Operations without tags go into a "_default" group.
    fn collect_operations(&self, api: &OpenAPI) -> HashMap<String, Vec<OperationInfo>> {
        let mut by_tag: HashMap<String, Vec<OperationInfo>> = HashMap::new();

        for (path_str, item) in &api.paths.paths {
            let item = match item {
                ReferenceOr::Item(item) => item,
                ReferenceOr::Reference { reference } => {
                    eprintln!(
                        "[poste import] warning: skipping $ref '{}' at path '{}'",
                        reference, path_str
                    );
                    continue;
                }
            };

            let operations = collect_methods(item);
            for (method, op) in operations {
                let tags = if op.tags.is_empty() {
                    vec!["_default".to_string()]
                } else {
                    op.tags.clone()
                };

                // Replace {param} with {{param}} for Poste variable syntax
                let http_path = path_str.replace('{', "{{").replace('}', "}}");

                for tag in &tags {
                    by_tag.entry(tag.clone()).or_default().push(OperationInfo {
                        method: method.to_uppercase(),
                        http_path: http_path.clone(),
                        operation_id: op.operation_id.clone().unwrap_or_else(|| {
                            format!(
                                "{}{}",
                                method.to_lowercase(),
                                sanitize_path_segment(path_str)
                            )
                        }),
                        summary: op.summary.clone().unwrap_or_default(),
                        parameters: op.parameters.clone(),
                        request_body: op.request_body.clone(),
                        security: op.security.clone(),
                    });
                }
            }
        }

        by_tag
    }

    /// Generate safe filename from tag name.
    fn tag_to_filename(tag: &str) -> String {
        let sanitized: String = tag
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if sanitized.is_empty() {
            "default".to_string()
        } else {
            sanitized.to_lowercase()
        }
    }
}

impl Default for OpenApiImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl SpecImporter for OpenApiImporter {
    fn import(&self, spec_content: &str) -> Result<ImportResult> {
        // Try JSON first, then YAML
        let api: OpenAPI = serde_json::from_str(spec_content)
            .or_else(|_| serde_yaml::from_str(spec_content))
            .context("Failed to parse OpenAPI spec (tried JSON and YAML)")?;

        let base_url = self.resolve_base_url(&api);
        let by_tag = self.collect_operations(&api);
        let mut files = Vec::new();
        let mut env_vars = HashMap::new();
        let mut warnings = Vec::new();

        env_vars.insert("base_url".to_string(), base_url.clone());

        // Build security scheme map: scheme_name → SecurityScheme
        let mut security_schemes: HashMap<String, &openapiv3::SecurityScheme> = HashMap::new();
        let global_security = api.security.as_ref();
        if let Some(ref components) = api.components {
            for (name, scheme) in &components.security_schemes {
                if let ReferenceOr::Item(s) = scheme {
                    security_schemes.insert(name.clone(), s);
                }
            }
        }

        // Process tags in sorted order for deterministic output
        let mut tag_names: Vec<&String> = by_tag.keys().collect();
        tag_names.sort();

        for tag in tag_names {
            let ops = &by_tag[tag];
            let filename = format!("{}.http", Self::tag_to_filename(tag));
            let mut content = String::new();

            // Collect file-level variable names (query, path, cookie, form body)
            // These become @var definitions at the file top instead of env.json entries
            let mut file_vars: Vec<(String, String)> = Vec::new();
            for op in ops {
                for param in &op.parameters {
                    if let ReferenceOr::Item(p) = param {
                        let name = match p {
                            Parameter::Query { parameter_data, .. } => {
                                sanitize_var_name(&parameter_data.name)
                            }
                            Parameter::Path { parameter_data, .. } => {
                                sanitize_var_name(&parameter_data.name)
                            }
                            Parameter::Cookie { parameter_data, .. } => {
                                sanitize_var_name(&parameter_data.name)
                            }
                            _ => continue,
                        };
                        if !file_vars.iter().any(|(n, _)| n == &name) {
                            let default = match p {
                                Parameter::Query { parameter_data, .. } => {
                                    extract_default_from_param(parameter_data)
                                }
                                Parameter::Path { parameter_data, .. } => {
                                    extract_default_from_param(parameter_data)
                                }
                                Parameter::Cookie { parameter_data, .. } => {
                                    extract_default_from_param(parameter_data)
                                }
                                _ => String::new(),
                            };
                            file_vars.push((name, default));
                        }
                    }
                }
                // Collect form body field names
                if let Some(body) = &op.request_body {
                    collect_form_body_fields(&api, body, &mut file_vars);
                }
            }

            // File-level base_url variable (resolved from env.json)
            content.push_str(&format!("@base_url = {}\n", poste_var("base_url")));
            // File-level non-header variables
            for (var_name, default) in &file_vars {
                if default.is_empty() {
                    content.push_str(&format!("@{} = \n", var_name));
                } else {
                    content.push_str(&format!("@{} = {}\n", var_name, default));
                }
            }
            if !file_vars.is_empty() {
                content.push('\n');
            }

            for op in ops {
                // Separator + name
                let display_name = if !op.operation_id.is_empty() {
                    op.operation_id.clone()
                } else {
                    format!("{} {}", op.method, &op.http_path)
                };
                content.push_str(&format!("### {}\n", display_name));
                if !op.summary.is_empty() {
                    content.push_str(&format!("# {}\n", op.summary));
                }

                // Add prompt directive lines for parameters with enum constraints
                for param in &op.parameters {
                    write_prompt_for_param(&mut content, &api, param);
                }

                // Collect query params for the request line
                let mut query_parts: Vec<String> = Vec::new();
                for param in &op.parameters {
                    if let ReferenceOr::Item(Parameter::Query { parameter_data, .. }) = param {
                        let var_name = sanitize_var_name(&parameter_data.name);
                        query_parts.push(format!(
                            "{}={}",
                            &parameter_data.name,
                            poste_var(&var_name)
                        ));
                    }
                }

                // Request line with query string
                let request_url = if query_parts.is_empty() {
                    format!("{{{{base_url}}}}{}", op.http_path)
                } else {
                    format!("{{{{base_url}}}}{}?{}", op.http_path, query_parts.join("&"))
                };
                content.push_str(&format!("{} {}\n", op.method, request_url));

                // Parameters: header/cookie become header lines
                for param in &op.parameters {
                    match param {
                        ReferenceOr::Item(Parameter::Header { parameter_data, .. }) => {
                            let var_name = sanitize_var_name(&parameter_data.name);
                            content.push_str(&format!(
                                "{}: {}\n",
                                parameter_data.name,
                                poste_var(&var_name)
                            ));
                            env_vars
                                .entry(var_name)
                                .or_insert_with(|| extract_default_from_param(parameter_data));
                        }
                        ReferenceOr::Item(Parameter::Query { .. }) => {
                            // Already handled in request line above
                        }
                        ReferenceOr::Item(Parameter::Path { .. }) => {
                            // Already collected as file-level @var
                        }
                        ReferenceOr::Item(Parameter::Cookie { parameter_data, .. }) => {
                            let var_name = sanitize_var_name(&parameter_data.name);
                            content.push_str(&format!(
                                "Cookie: {}={}\n",
                                parameter_data.name,
                                poste_var(&var_name)
                            ));
                        }
                        ReferenceOr::Reference { reference } => {
                            warnings.push(format!("Skipping $ref parameter: {}", reference));
                        }
                    }
                }

                // Security: inject auth headers based on operation or global security
                #[allow(clippy::option_as_ref_deref)]
                let op_security = op
                    .security
                    .as_ref()
                    .map(|s| s.as_slice())
                    .or_else(|| global_security.map(|s| s.as_slice()));
                if let Some(security_reqs) = op_security {
                    for req in security_reqs {
                        for (scheme_name, _scopes) in req {
                            if let Some(scheme) = security_schemes.get(scheme_name) {
                                match scheme {
                                    openapiv3::SecurityScheme::APIKey {
                                        location, name, ..
                                    } => match location {
                                        openapiv3::APIKeyLocation::Header => {
                                            let var_name = sanitize_var_name(name);
                                            content.push_str(&format!(
                                                "{}: {}\n",
                                                name,
                                                poste_var(&var_name)
                                            ));
                                            env_vars.entry(var_name).or_default();
                                        }
                                        openapiv3::APIKeyLocation::Query => {
                                            let var_name = sanitize_var_name(name);
                                            env_vars.entry(var_name).or_default();
                                        }
                                        openapiv3::APIKeyLocation::Cookie => {
                                            let var_name = sanitize_var_name(name);
                                            content.push_str(&format!(
                                                "Cookie: {}={}\n",
                                                name,
                                                poste_var(&var_name)
                                            ));
                                            env_vars.entry(var_name).or_default();
                                        }
                                    },
                                    openapiv3::SecurityScheme::HTTP { scheme, .. } => {
                                        match scheme.to_lowercase().as_str() {
                                            "bearer" => {
                                                content.push_str(
                                                    "Authorization: Bearer {{auth_token}}\n",
                                                );
                                                env_vars.entry("auth_token".into()).or_default();
                                            }
                                            "basic" => {
                                                content.push_str(
                                                    "Authorization: Basic {{basic_auth}}\n",
                                                );
                                                env_vars.entry("basic_auth".into()).or_default();
                                            }
                                            other => {
                                                let var_name = format!("auth_{}", other);
                                                content.push_str(&format!(
                                                    "Authorization: {other} {}\n",
                                                    poste_var(&var_name)
                                                ));
                                                env_vars.entry(var_name).or_default();
                                            }
                                        }
                                    }
                                    _ => {
                                        // OAuth2 / OpenIDConnect — placeholder
                                        let var_name =
                                            format!("{}_token", sanitize_var_name(scheme_name));
                                        content.push_str(&format!(
                                            "# {} auth required\n",
                                            scheme_name
                                        ));
                                        env_vars.entry(var_name).or_default();
                                    }
                                }
                            } else {
                                warnings.push(format!(
                                    "Security scheme '{}' not found in components.securitySchemes",
                                    scheme_name
                                ));
                            }
                        }
                    }
                }

                // Request body
                if let Some(body) = &op.request_body {
                    let body_item = match body {
                        ReferenceOr::Item(item) => Some(item),
                        ReferenceOr::Reference { reference } => {
                            match resolve_ref_request_body(&api, reference) {
                                Some(resolved) => Some(resolved),
                                None => {
                                    warnings.push(format!(
                                        "Could not resolve request body $ref: {}",
                                        reference
                                    ));
                                    None
                                }
                            }
                        }
                    };

                    if let Some(body_item) = body_item {
                        if let Some((content_type, media_type)) = body_item.content.iter().next() {
                            content.push_str(&format!("Content-Type: {}\n", content_type));
                            content.push('\n');

                            let mut wrote_body = false;

                            // 1. Try media type example
                            if let Some(example) = &media_type.example {
                                if let Ok(json) = serde_json::to_string_pretty(example) {
                                    content.push_str(&json);
                                    content.push('\n');
                                    wrote_body = true;
                                }
                            }

                            // 2. Try schema-level example (resolving $ref if needed)
                            if !wrote_body {
                                if let Some(schema) = &media_type.schema {
                                    if let Some(s) = get_schema(&api, schema) {
                                        if let Some(ex) = &s.schema_data.example {
                                            if let Ok(json) = serde_json::to_string_pretty(ex) {
                                                content.push_str(&json);
                                                content.push('\n');
                                                wrote_body = true;
                                            }
                                        }
                                    }
                                }
                            }

                            // 3. Generate body from schema when no example
                            if !wrote_body {
                                if let Some(schema) = &media_type.schema {
                                    if let Some(s) = get_schema(&api, schema) {
                                        match content_type.as_str() {
                                            "application/x-www-form-urlencoded" => {
                                                write_form_body(&mut content, s, &api);
                                            }
                                            "application/json" | "application/xml" => {
                                                if let Some(json) =
                                                    generate_json_skeleton(s, &api, 0)
                                                {
                                                    content.push_str(&json);
                                                    content.push('\n');
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                content.push('\n');
            }

            files.push(HttpFile {
                path: filename,
                content,
            });
        }

        Ok(ImportResult {
            files,
            env_vars,
            warnings,
        })
    }
}

// ---------------------------------------------------------------------------
// Internal types and helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct OperationInfo {
    method: String,
    http_path: String,
    operation_id: String,
    summary: String,
    parameters: Vec<openapiv3::ReferenceOr<openapiv3::Parameter>>,
    request_body: Option<openapiv3::ReferenceOr<openapiv3::RequestBody>>,
    security: Option<Vec<openapiv3::SecurityRequirement>>,
}

/// Collect (method_name, Operation) pairs from a PathItem.
fn collect_methods(item: &PathItem) -> Vec<(&str, &Operation)> {
    let mut ops = Vec::new();
    if let Some(ref op) = item.get {
        ops.push(("GET", op));
    }
    if let Some(ref op) = item.post {
        ops.push(("POST", op));
    }
    if let Some(ref op) = item.put {
        ops.push(("PUT", op));
    }
    if let Some(ref op) = item.delete {
        ops.push(("DELETE", op));
    }
    if let Some(ref op) = item.patch {
        ops.push(("PATCH", op));
    }
    if let Some(ref op) = item.options {
        ops.push(("OPTIONS", op));
    }
    if let Some(ref op) = item.head {
        ops.push(("HEAD", op));
    }
    if let Some(ref op) = item.trace {
        ops.push(("TRACE", op));
    }
    ops
}

/// Wrap a variable name in Poste's {{var}} syntax.
fn poste_var(name: &str) -> String {
    format!("{{{{{}}}}}", name)
}

/// Create a safe variable name from a parameter name.
fn sanitize_var_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c == '-' || c == '.' || c == ' ' {
                '_'
            } else {
                c
            }
        })
        .collect();
    if s.is_empty() {
        "param".to_string()
    } else {
        s
    }
}

/// Create a safe path segment for fallback operationId generation.
fn sanitize_path_segment(path: &str) -> String {
    path.trim_start_matches('/')
        .replace('/', "_")
        .replace(['{', '}'], "")
        .replace('-', "_")
}

/// Extract a default value string from a parameter's schema.
/// Also handles array schemas with items that have defaults.
fn extract_default_from_param(data: &openapiv3::ParameterData) -> String {
    match &data.format {
        ParameterSchemaOrContent::Schema(schema_ref) => {
            match schema_ref {
                ReferenceOr::Item(schema) => {
                    // Try direct default
                    if let Some(default) = &schema.schema_data.default {
                        if let Ok(s) = serde_json::to_string(default) {
                            return s.trim_matches('"').to_string();
                        }
                    }
                    // Try items default for array schemas
                    if let SchemaKind::Type(Type::Array(arr)) = &schema.schema_kind {
                        if let Some(items) = &arr.items {
                            if let Some(item_schema) = items.as_item() {
                                if let Some(default) = &item_schema.schema_data.default {
                                    if let Ok(s) = serde_json::to_string(default) {
                                        return s.trim_matches('"').to_string();
                                    }
                                }
                            }
                        }
                    }
                    String::new()
                }
                ReferenceOr::Reference { .. } => String::new(),
            }
        }
        ParameterSchemaOrContent::Content(_) => String::new(),
    }
}

/// Resolve a `$ref` string to a request body from the spec's components.
fn resolve_ref_request_body<'a>(api: &'a OpenAPI, reference: &str) -> Option<&'a RequestBody> {
    let path = reference.strip_prefix("#/components/")?;
    let (component_type, name) = path.split_once('/')?;
    match component_type {
        "requestBodies" => api.components.as_ref()?.request_bodies.get(name)?.as_item(),
        _ => None,
    }
}

/// Resolve a `$ref` string to a schema from the spec's components.
fn resolve_ref_schema<'a>(api: &'a OpenAPI, reference: &str) -> Option<&'a Schema> {
    let path = reference.strip_prefix("#/components/")?;
    let (component_type, name) = path.split_once('/')?;
    match component_type {
        "schemas" => api.components.as_ref()?.schemas.get(name)?.as_item(),
        _ => None,
    }
}

/// Get a concrete Schema from a ReferenceOr, following $ref if needed.
fn get_schema<'a>(api: &'a OpenAPI, s: &'a ReferenceOr<Schema>) -> Option<&'a Schema> {
    match s {
        ReferenceOr::Item(schema) => Some(schema),
        ReferenceOr::Reference { reference } => resolve_ref_schema(api, reference),
    }
}

/// Extract string enum values from a schema, if present.
/// Handles typed schemas (String, Array→items) and AnySchema with enumeration.
fn extract_enum_values(schema: &Schema) -> Option<Vec<String>> {
    match &schema.schema_kind {
        SchemaKind::Type(Type::String(s)) => {
            let vals: Vec<String> = s.enumeration.iter().filter_map(|e| e.clone()).collect();
            if !vals.is_empty() {
                Some(vals)
            } else {
                None
            }
        }
        SchemaKind::Type(Type::Array(arr)) => {
            arr.items.as_ref().and_then(|items| match items.as_item() {
                Some(item) => extract_enum_values(item),
                None => None,
            })
        }
        SchemaKind::Any(any) => {
            let vals: Vec<String> = any
                .enumeration
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            if !vals.is_empty() {
                Some(vals)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Write a `<<name` prompt directive line for a parameter with enum constraints.
fn write_prompt_for_param(content: &mut String, api: &OpenAPI, param: &ReferenceOr<Parameter>) {
    if let ReferenceOr::Item(param) = param {
        let (var_name, schema_ref) = match param {
            Parameter::Query { parameter_data, .. } => (
                parameter_data.name.clone(),
                match &parameter_data.format {
                    ParameterSchemaOrContent::Schema(s) => Some(s),
                    ParameterSchemaOrContent::Content(_) => None,
                },
            ),
            Parameter::Path { parameter_data, .. } => (
                parameter_data.name.clone(),
                match &parameter_data.format {
                    ParameterSchemaOrContent::Schema(s) => Some(s),
                    ParameterSchemaOrContent::Content(_) => None,
                },
            ),
            _ => return,
        };

        if let Some(schema_ref) = schema_ref {
            if let Some(schema) = get_schema(api, schema_ref) {
                if let Some(values) = extract_enum_values(schema) {
                    let sanitized = sanitize_var_name(&var_name);
                    content.push_str(&format!("<<{} [{}]\n", sanitized, values.join(", ")));
                }
            }
        }
    }
}

/// Generate a JSON value for a schema property, using example/default/enum when available.
fn skeleton_value_for_schema(schema: &Schema, api: &OpenAPI, depth: usize) -> serde_json::Value {
    // Priority 1: schema-level example
    if let Some(example) = &schema.schema_data.example {
        return example.clone();
    }
    // Priority 2: schema-level default
    if let Some(default) = &schema.schema_data.default {
        return default.clone();
    }

    // Priority 3: enum-based or type-based default
    match &schema.schema_kind {
        SchemaKind::Type(Type::String(s)) => {
            if let Some(first) = s.enumeration.iter().find_map(|e| e.clone()) {
                serde_json::Value::String(first)
            } else {
                serde_json::Value::String(String::new())
            }
        }
        SchemaKind::Type(Type::Integer(i)) => {
            if let Some(first) = i.enumeration.first().and_then(|e| *e) {
                serde_json::Value::Number(serde_json::Number::from(first))
            } else {
                serde_json::Value::Number(serde_json::Number::from(0))
            }
        }
        SchemaKind::Type(Type::Number(n)) => {
            if let Some(first) = n.enumeration.first().and_then(|e| *e) {
                serde_json::Value::Number(
                    serde_json::Number::from_f64(first)
                        .unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap()),
                )
            } else {
                serde_json::Value::Number(serde_json::Number::from_f64(0.0).unwrap())
            }
        }
        SchemaKind::Type(Type::Boolean(b)) => {
            if let Some(first) = b.enumeration.first().and_then(|e| *e) {
                serde_json::Value::Bool(first)
            } else {
                serde_json::Value::Bool(false)
            }
        }
        SchemaKind::Type(Type::Array(arr)) => {
            let mut items = Vec::new();
            if let Some(item_ref) = &arr.items {
                match item_ref {
                    ReferenceOr::Item(item_schema) => {
                        items.push(skeleton_value_for_schema(item_schema, api, depth + 1));
                    }
                    ReferenceOr::Reference { reference } => {
                        if let Some(resolved) = resolve_ref_schema(api, reference) {
                            items.push(skeleton_value_for_schema(resolved, api, depth + 1));
                        }
                    }
                }
            }
            serde_json::Value::Array(items)
        }
        SchemaKind::Type(Type::Object(_)) => {
            if depth <= 2 {
                let json =
                    generate_json_skeleton(schema, api, depth).unwrap_or_else(|| "{}".to_string());
                serde_json::from_str(&json)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
            } else {
                serde_json::Value::Object(serde_json::Map::new())
            }
        }
        SchemaKind::Any(any) => {
            // Check enum values first
            if let Some(first) = any.enumeration.first() {
                return first.clone();
            }
            // Check if it has object properties
            if !any.properties.is_empty() && depth <= 2 {
                let json =
                    generate_json_skeleton(schema, api, depth).unwrap_or_else(|| "{}".to_string());
                serde_json::from_str(&json)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
            } else {
                serde_json::Value::Null
            }
        }
        _ => serde_json::Value::Null,
    }
}

/// Generate a JSON body skeleton from an object schema.
fn generate_json_skeleton(schema: &Schema, api: &OpenAPI, depth: usize) -> Option<String> {
    if depth > 2 {
        return Some("{}".to_string());
    }

    let properties = match &schema.schema_kind {
        SchemaKind::Type(Type::Object(ObjectType { properties, .. })) => properties,
        SchemaKind::Any(any) => &any.properties,
        _ => return Some("{}".to_string()),
    };

    if properties.is_empty() {
        return Some("{}".to_string());
    }

    let mut map = serde_json::Map::new();
    for (name, prop) in properties.iter().take(8) {
        let val = match prop {
            ReferenceOr::Item(prop_schema) => skeleton_value_for_schema(prop_schema, api, depth),
            ReferenceOr::Reference { reference } => {
                if let Some(resolved) = resolve_ref_schema(api, reference) {
                    skeleton_value_for_schema(resolved, api, depth)
                } else {
                    serde_json::Value::Null
                }
            }
        };
        map.insert(name.clone(), val);
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(map)).ok()
}

/// Write form-urlencoded body from schema properties.
fn write_form_body(content: &mut String, schema: &Schema, _api: &OpenAPI) {
    let properties = match &schema.schema_kind {
        SchemaKind::Type(Type::Object(ObjectType { properties, .. })) => properties,
        SchemaKind::Any(any) => &any.properties,
        _ => return,
    };

    if properties.is_empty() {
        return;
    }

    let mut parts = Vec::new();
    for (name, _prop) in properties.iter() {
        let var_name = sanitize_var_name(name);
        parts.push(format!("{}={}", name, poste_var(&var_name)));
    }
    content.push_str(&parts.join("&"));
    content.push('\n');
}

/// Collect form body field names from a request body schema for file-level @var definitions.
fn collect_form_body_fields(
    api: &OpenAPI,
    body: &ReferenceOr<RequestBody>,
    file_vars: &mut Vec<(String, String)>,
) {
    let body_item = match body {
        ReferenceOr::Item(item) => item,
        ReferenceOr::Reference { reference } => match resolve_ref_request_body(api, reference) {
            Some(item) => item,
            None => return,
        },
    };

    for (content_type, media_type) in &body_item.content {
        if content_type != "application/x-www-form-urlencoded" {
            continue;
        }
        if let Some(schema) = &media_type.schema {
            if let Some(s) = get_schema(api, schema) {
                let properties = match &s.schema_kind {
                    SchemaKind::Type(Type::Object(ObjectType { properties, .. })) => properties,
                    SchemaKind::Any(any) => &any.properties,
                    _ => return,
                };
                for (name, _prop) in properties.iter() {
                    let var_name = sanitize_var_name(name);
                    if !file_vars.iter().any(|(n, _)| n == &var_name) {
                        file_vars.push((var_name, String::new()));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse OpenAPI JSON and run import, return single file content.
    fn import_one(spec: &str, base_url: &str) -> ImportResult {
        let importer = OpenApiImporter::with_base_url(base_url);
        importer.import(spec).unwrap()
    }

    // -----------------------------------------------------------------------
    // Step 3a: Single GET endpoint
    // -----------------------------------------------------------------------

    #[test]
    fn test_single_get_endpoint() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Petstore", "version": "1.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "summary": "List all pets",
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "pets.http");
        let c = &result.files[0].content;
        assert!(c.contains("### listPets"), "request name: {}", c);
        assert!(c.contains("GET {{base_url}}/pets"), "request line: {}", c);
        assert!(c.contains("List all pets"), "summary: {}", c);
    }

    // -----------------------------------------------------------------------
    // Step 3b: Multiple tags → multiple files
    // -----------------------------------------------------------------------

    #[test]
    fn test_multiple_tags_multiple_files() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Multi", "version": "1.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "responses": { "200": { "description": "OK" } }
                    }
                },
                "/store/inventory": {
                    "get": {
                        "tags": ["store"],
                        "operationId": "getInventory",
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        assert_eq!(result.files.len(), 2);
        let paths: Vec<&str> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.contains(&"pets.http"),
            "should have pets.http: {:?}",
            paths
        );
        assert!(
            paths.contains(&"store.http"),
            "should have store.http: {:?}",
            paths
        );
    }

    // -----------------------------------------------------------------------
    // Step 3c: Path parameters {petId} → {{petId}}
    // -----------------------------------------------------------------------

    #[test]
    fn test_path_parameters() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Petstore", "version": "1.0" },
            "paths": {
                "/pets/{petId}": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "getPetById",
                        "parameters": [
                            { "name": "petId", "in": "path", "required": true, "schema": { "type": "integer" } }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("{{petId}}"),
            "path param should be {{var}}: {}",
            c
        );
        // Also check that the raw path template doesn't leak through
        assert!(
            !c.contains("/pets/{petId}"),
            "raw path should be replaced: {}",
            c
        );
        // Should also have @petId at file top as file-level @var
        assert!(
            c.contains("@petId ="),
            "file should have @petId @var: {}",
            c
        );
        // Path param should NOT be in env_vars (it's file-level now)
        assert!(
            !result.env_vars.contains_key("petId"),
            "petId should not be in env_vars"
        );
    }

    // -----------------------------------------------------------------------
    // Step 3d: Empty spec returns empty result
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_openapi_creates_no_files() {
        let spec = r#"{"openapi":"3.0.0","info":{"title":"Empty","version":"1.0"},"paths":{}}"#;
        let result = import_one(spec, "http://localhost");
        assert!(result.files.is_empty());
    }

    // -----------------------------------------------------------------------
    // Step 3e: Variables in env_vars
    // -----------------------------------------------------------------------

    #[test]
    fn test_base_url_in_env_vars() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {}
        }"#;
        let importer = OpenApiImporter::with_base_url("https://my.api.com/v2");
        let result = importer.import(spec).unwrap();
        assert_eq!(
            result.env_vars.get("base_url").unwrap(),
            "https://my.api.com/v2"
        );
    }

    // -----------------------------------------------------------------------
    // Step 3f: No tag → "_default" file
    // -----------------------------------------------------------------------

    #[test]
    fn test_no_tag_uses_default_file() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/health": {
                    "get": {
                        "operationId": "healthCheck",
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "http://localhost");
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "_default.http");
    }

    // -----------------------------------------------------------------------
    // Step 3g: Tag name sanitization
    // -----------------------------------------------------------------------

    #[test]
    fn test_tag_to_filename_sanitization() {
        assert_eq!(
            OpenApiImporter::tag_to_filename("User Management"),
            "user_management"
        );
        assert_eq!(OpenApiImporter::tag_to_filename("Pets"), "pets");
        assert_eq!(OpenApiImporter::tag_to_filename(""), "default");
    }

    // -----------------------------------------------------------------------
    // Step 4a: Query parameters appear on request line
    // -----------------------------------------------------------------------

    #[test]
    fn test_query_params_on_request_line() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "parameters": [
                            { "name": "limit", "in": "query", "schema": { "type": "integer" } },
                            { "name": "status", "in": "query", "schema": { "type": "string" } }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(c.contains("?limit={{limit}}"), "query on URL: {}", c);
        assert!(
            c.contains("status={{status}}"),
            "second query on URL: {}",
            c
        );
        // Query params should be @var at file top, not in env_vars
        assert!(c.contains("@limit ="), "file should have @limit: {}", c);
        assert!(c.contains("@status ="), "file should have @status: {}", c);
        assert!(
            !result.env_vars.contains_key("limit"),
            "limit not in env_vars"
        );
        assert!(
            !result.env_vars.contains_key("status"),
            "status not in env_vars"
        );
    }

    // -----------------------------------------------------------------------
    // Step 4b: Header parameters
    // -----------------------------------------------------------------------

    #[test]
    fn test_header_params_as_headers() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "parameters": [
                            { "name": "X-Request-Id", "in": "header", "schema": { "type": "string" } }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(c.contains("X-Request-Id"), "header name: {}", c);
        assert!(
            result.env_vars.contains_key("X_Request_Id"),
            "header var should be X_Request_Id"
        );
    }

    // -----------------------------------------------------------------------
    // Step 4c: Cookie parameters
    // -----------------------------------------------------------------------

    #[test]
    fn test_cookie_params_as_cookie_header() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "parameters": [
                            { "name": "session_id", "in": "cookie", "schema": { "type": "string" } }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(c.contains("Cookie:"), "Cookie header: {}", c);
        // Cookie param should be @var at file top, not in env_vars
        assert!(
            c.contains("@session_id ="),
            "file should have @session_id: {}",
            c
        );
        assert!(
            !result.env_vars.contains_key("session_id"),
            "cookie var not in env"
        );
    }

    // -----------------------------------------------------------------------
    // Step 5a: Request body with example
    // -----------------------------------------------------------------------

    #[test]
    fn test_request_body_with_example() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/pets": {
                    "post": {
                        "tags": ["pets"],
                        "operationId": "createPet",
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "example": { "name": "Fluffy", "type": "cat" }
                                }
                            }
                        },
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("Content-Type: application/json"),
            "content type: {}",
            c
        );
        assert!(c.contains("Fluffy"), "example value: {}", c);
        assert!(c.contains("cat"), "example value: {}", c);
    }

    // -----------------------------------------------------------------------
    // Step 5b: Request body with schema-level example (fallback)
    // -----------------------------------------------------------------------

    #[test]
    fn test_request_body_schema_example_fallback() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/pets": {
                    "post": {
                        "tags": ["pets"],
                        "operationId": "createPet",
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "name": { "type": "string" }
                                        },
                                        "example": { "name": "Doggo" }
                                    }
                                }
                            }
                        },
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("Content-Type: application/json"),
            "content type: {}",
            c
        );
        assert!(c.contains("Doggo"), "schema example: {}", c);
    }

    // -----------------------------------------------------------------------
    // Step 5c: Request body with $ref — now resolved correctly
    // -----------------------------------------------------------------------

    #[test]
    fn test_request_body_ref_resolved() {
        let spec = r##"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "components": {
                "requestBodies": {
                    "PetBody": {
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/Pet"
                                }
                            }
                        },
                        "required": true
                    }
                },
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "age": { "type": "integer" }
                        }
                    }
                }
            },
            "paths": {
                "/pets": {
                    "post": {
                        "tags": ["pets"],
                        "operationId": "createPet",
                        "requestBody": {
                            "$ref": "#/components/requestBodies/PetBody"
                        },
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"##;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(c.contains("### createPet"), "should have request: {}", c);
        assert!(c.contains("POST"), "should have method: {}", c);
        assert!(
            c.contains("Content-Type: application/json"),
            "resolved content type: {}",
            c
        );
        assert!(c.contains("\"name\""), "schema properties: {}", c);
        assert!(c.contains("\"age\""), "schema properties: {}", c);
    }

    // -----------------------------------------------------------------------
    // Step 5d: Multipart form-data content type
    // -----------------------------------------------------------------------

    #[test]
    fn test_request_body_multipart() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/upload": {
                    "post": {
                        "tags": ["upload"],
                        "operationId": "uploadFile",
                        "requestBody": {
                            "content": {
                                "multipart/form-data": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "file": { "type": "string", "format": "binary" }
                                        }
                                    }
                                }
                            }
                        },
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("Content-Type: multipart/form-data"),
            "content type: {}",
            c
        );
    }

    // -----------------------------------------------------------------------
    // Step 5e: prompt for query parameter with enum values
    // -----------------------------------------------------------------------

    #[test]
    fn test_enum_param_prompt_directive() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "parameters": [
                            {
                                "name": "status",
                                "in": "query",
                                "schema": {
                                    "type": "string",
                                    "enum": ["available", "pending", "sold"]
                                }
                            }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("<<status [available, pending, sold]"),
            "prompt directive: {}",
            c
        );
    }

    // -----------------------------------------------------------------------
    // Step 5f: Form-urlencoded body from schema properties
    // -----------------------------------------------------------------------

    #[test]
    fn test_form_urlencoded_body() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/pets/{petId}": {
                    "post": {
                        "tags": ["pets"],
                        "operationId": "updatePet",
                        "parameters": [
                            { "name": "petId", "in": "path", "required": true, "schema": { "type": "integer" } }
                        ],
                        "requestBody": {
                            "content": {
                                "application/x-www-form-urlencoded": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "name": { "type": "string" },
                                            "status": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        },
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("Content-Type: application/x-www-form-urlencoded"),
            "content type: {}",
            c
        );
        assert!(c.contains("name={{name}}"), "form field: {}", c);
        assert!(c.contains("status={{status}}"), "form field: {}", c);
        // Form body fields should be @var at file top, not in env_vars
        assert!(c.contains("@name ="), "file should have @name: {}", c);
        assert!(c.contains("@status ="), "file should have @status: {}", c);
        assert!(
            !result.env_vars.contains_key("name"),
            "name not in env_vars"
        );
        assert!(
            !result.env_vars.contains_key("status"),
            "status not in env_vars"
        );
    }

    // -----------------------------------------------------------------------
    // Step 5g: JSON skeleton from schema with no example
    // -----------------------------------------------------------------------

    #[test]
    fn test_json_skeleton_generated() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/orders": {
                    "post": {
                        "tags": ["orders"],
                        "operationId": "placeOrder",
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "quantity": { "type": "integer" },
                                            "status": { "type": "string" },
                                            "complete": { "type": "boolean" }
                                        }
                                    }
                                }
                            }
                        },
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("Content-Type: application/json"),
            "content type: {}",
            c
        );
        assert!(c.contains("\"quantity\": 0"), "default integer: {}", c);
        assert!(c.contains("\"status\": \"\""), "default string: {}", c);
        assert!(c.contains("\"complete\": false"), "default bool: {}", c);
    }

    // -----------------------------------------------------------------------
    // Step 5h: Array items default extracted for parameter
    // -----------------------------------------------------------------------

    #[test]
    fn test_array_items_default_extracted() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "parameters": [
                            {
                                "name": "status",
                                "in": "query",
                                "required": true,
                                "schema": {
                                    "type": "array",
                                    "items": {
                                        "type": "string",
                                        "enum": ["available", "pending", "sold"],
                                        "default": "available"
                                    }
                                }
                            }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        // Default should be in @var at file top, not in env_vars
        assert!(
            result.files[0].content.contains("@status = available"),
            "file should have @status = available: {}",
            result.files[0].content
        );
        assert!(
            !result.env_vars.contains_key("status"),
            "status not in env_vars"
        );
    }

    // -----------------------------------------------------------------------
    // Step 6a: API Key auth → header + env var
    // -----------------------------------------------------------------------

    #[test]
    fn test_api_key_auth_header() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "components": {
                "securitySchemes": {
                    "api_key": {
                        "type": "apiKey",
                        "in": "header",
                        "name": "X-API-Key"
                    }
                }
            },
            "security": [ { "api_key": [] } ],
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("X-API-Key: {{X_API_Key}}"),
            "api key header: {}",
            c
        );
        assert!(result.env_vars.contains_key("X_API_Key"), "api key in env");
    }

    // -----------------------------------------------------------------------
    // Step 6b: Bearer auth → Authorization header
    // -----------------------------------------------------------------------

    #[test]
    fn test_bearer_auth_header() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "components": {
                "securitySchemes": {
                    "bearer_auth": {
                        "type": "http",
                        "scheme": "bearer"
                    }
                }
            },
            "security": [ { "bearer_auth": [] } ],
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "listPets",
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        assert!(
            c.contains("Authorization: Bearer {{auth_token}}"),
            "bearer auth: {}",
            c
        );
        assert!(
            result.env_vars.contains_key("auth_token"),
            "auth_token in env"
        );
    }

    // -----------------------------------------------------------------------
    // Step 6c: Operation-level security overrides global
    // -----------------------------------------------------------------------

    #[test]
    fn test_operation_security_override() {
        let spec = r#"{
            "openapi": "3.0.0",
            "info": { "title": "Test", "version": "1.0" },
            "components": {
                "securitySchemes": {
                    "api_key": {
                        "type": "apiKey",
                        "in": "header",
                        "name": "X-API-Key"
                    }
                }
            },
            "paths": {
                "/public": {
                    "get": {
                        "tags": ["public"],
                        "operationId": "publicEndpoint",
                        "security": [],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let result = import_one(spec, "https://api.example.com");
        let c = &result.files[0].content;
        // Empty security = no auth header
        assert!(!c.contains("X-API-Key"), "no auth for public endpoint");
    }
}
