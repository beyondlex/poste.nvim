use crate::import::{ImportResult, SpecImporter};
use anyhow::{Context, Result};
use serde_json::Value;

/// Import from Swagger 2.0 spec.
/// Internally converts to OpenAPI 3.x and delegates to OpenApiImporter.
pub struct SwaggerImporter;

impl SpecImporter for SwaggerImporter {
    fn import(&self, spec_content: &str) -> Result<ImportResult> {
        let oas3_json = swagger_to_openapi3(spec_content)?;
        let importer = super::openapi::OpenApiImporter::new();
        importer.import(&oas3_json)
    }
}

/// Convert Swagger 2.0 JSON to OpenAPI 3.x JSON string.
pub fn swagger_to_openapi3(spec: &str) -> Result<String> {
    let swagger: Value = serde_json::from_str(spec)
        .or_else(|_| serde_yaml::from_str(spec))
        .context("Failed to parse Swagger spec (tried JSON and YAML)")?;

    // Verify it's Swagger 2.0
    let swagger_ver = swagger
        .get("swagger")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if swagger_ver != "2.0" {
        anyhow::bail!("Expected Swagger 2.0, got swagger={}", swagger_ver);
    }

    let mut out = serde_json::Map::new();

    // openapi version
    out.insert("openapi".to_string(), Value::String("3.0.0".to_string()));

    // info
    if let Some(info) = swagger.get("info") {
        // Ensure info has a version field (required in OAS3)
        let mut info = info.clone();
        if let Some(obj) = info.as_object_mut() {
            if !obj.contains_key("version") {
                obj.insert("version".to_string(), Value::String("1.0".to_string()));
            }
        }
        out.insert("info".to_string(), info);
    }

    // servers: host + basePath + schemes
    let host = swagger
        .get("host")
        .and_then(|v| v.as_str())
        .unwrap_or("localhost");
    let base_path = swagger
        .get("basePath")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let schemes = swagger
        .get("schemes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec!["https".to_string()]);
    let protocol = if schemes.contains(&"https".to_string()) {
        "https"
    } else {
        "http"
    };
    let url = format!("{}://{}{}", protocol, host, base_path);
    let server = serde_json::json!({ "url": url });
    out.insert("servers".to_string(), Value::Array(vec![server]));

    // paths: convert parameters
    let mut paths = serde_json::Map::new();
    if let Some(swagger_paths) = swagger.get("paths").and_then(|v| v.as_object()) {
        for (path_str, path_item) in swagger_paths {
            if let Some(methods) = path_item.as_object() {
                let mut new_path = serde_json::Map::new();
                for (method, operation) in methods {
                    if method == "parameters" {
                        // Path-level parameters
                        if let Some(params) = operation.as_array() {
                            new_path.insert(
                                "parameters".to_string(),
                                Value::Array(convert_parameters(params)),
                            );
                        }
                    } else if let Some(op_obj) = operation.as_object() {
                        let mut new_op = op_obj.clone();
                        // Convert body/formData parameters to requestBody
                        let body_params: Vec<&Value> = op_obj
                            .get("parameters")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().collect())
                            .unwrap_or_default();

                        let has_body = body_params
                            .iter()
                            .any(|p| p.get("in").and_then(|v| v.as_str()) == Some("body"));
                        let has_form = body_params
                            .iter()
                            .any(|p| p.get("in").and_then(|v| v.as_str()) == Some("formData"));

                        if has_body || has_form {
                            let mut content = serde_json::Map::new();
                            if has_body {
                                if let Some(body_param) = body_params
                                    .iter()
                                    .find(|p| p.get("in").and_then(|v| v.as_str()) == Some("body"))
                                {
                                    if let Some(schema) = body_param.get("schema") {
                                        let mt = serde_json::json!({
                                            "schema": schema
                                        });
                                        content.insert("application/json".to_string(), mt);
                                    }
                                }
                            }
                            if has_form {
                                let mut form_props = serde_json::Map::new();
                                let mut required = Vec::new();
                                for p in &body_params {
                                    if p.get("in").and_then(|v| v.as_str()) == Some("formData") {
                                        let name = p
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("field");
                                        let p_type = p
                                            .get("type")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("string");
                                        let prop = serde_json::json!({
                                            "type": p_type
                                        });
                                        form_props.insert(name.to_string(), prop);
                                        if p.get("required")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false)
                                        {
                                            required.push(Value::String(name.to_string()));
                                        }
                                    }
                                }
                                let mut form_schema = serde_json::Map::new();
                                form_schema.insert(
                                    "type".to_string(),
                                    Value::String("object".to_string()),
                                );
                                form_schema
                                    .insert("properties".to_string(), Value::Object(form_props));
                                if !required.is_empty() {
                                    form_schema
                                        .insert("required".to_string(), Value::Array(required));
                                }
                                let mt = serde_json::json!({
                                    "schema": Value::Object(form_schema)
                                });
                                content.insert("multipart/form-data".to_string(), mt);
                            }
                            let request_body = serde_json::json!({
                                "content": content
                            });
                            new_op.insert("requestBody".to_string(), request_body);
                        }

                        // Convert non-body/formData parameters
                        let remaining_params: Vec<Value> = body_params
                            .iter()
                            .filter(|p| {
                                let loc = p.get("in").and_then(|v| v.as_str()).unwrap_or("");
                                loc != "body" && loc != "formData"
                            })
                            .copied()
                            .map(convert_swagger_param_to_oas3)
                            .collect();

                        if !remaining_params.is_empty() {
                            new_op.insert("parameters".to_string(), Value::Array(remaining_params));
                        } else {
                            new_op.remove("parameters");
                        }

                        new_path.insert(method.clone(), Value::Object(new_op));
                    }
                }
                paths.insert(path_str.clone(), Value::Object(new_path));
            }
        }
    }
    out.insert("paths".to_string(), Value::Object(paths));

    // definitions → components.schemas
    if let Some(defs) = swagger.get("definitions") {
        let mut components = serde_json::Map::new();
        components.insert("schemas".to_string(), defs.clone());
        out.insert("components".to_string(), Value::Object(components));
    }

    // securityDefinitions → components.securitySchemes
    if let Some(sec_defs) = swagger.get("securityDefinitions") {
        let components = out
            .entry("components".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(obj) = components.as_object_mut() {
            obj.insert(
                "securitySchemes".to_string(),
                convert_security_definitions(sec_defs),
            );
        }
    }

    // security (same structure)
    if let Some(sec) = swagger.get("security") {
        out.insert("security".to_string(), sec.clone());
    }

    Ok(serde_json::to_string_pretty(&Value::Object(out))?)
}

/// Convert Swagger 2.0 parameter list to OAS3 parameter list (for non-body params).
fn convert_parameters(params: &[Value]) -> Vec<Value> {
    params
        .iter()
        .filter(|p| {
            let loc = p.get("in").and_then(|v| v.as_str()).unwrap_or("");
            loc != "body" && loc != "formData"
        })
        .map(convert_swagger_param_to_oas3)
        .collect()
}

/// Convert a single Swagger 2.0 parameter to OAS3 format.
fn convert_swagger_param_to_oas3(param: &Value) -> Value {
    let mut oas3 = param.clone();
    if let Some(obj) = oas3.as_object_mut() {
        // Remove Swagger-specific fields
        obj.remove("in"); // Keep "in" — same in OAS3
                          // Move schema info to "schema" object
        let p_type = obj.remove("type");
        let p_format = obj.remove("format");
        let p_items = obj.remove("items");
        let _p_collection_format = obj.remove("collectionFormat");
        let p_default = obj.remove("default");

        let mut schema = serde_json::Map::new();
        if let Some(t) = p_type {
            if let Some(s) = t.as_str() {
                // Map Swagger types to OAS3
                match s {
                    "integer" | "number" | "string" | "boolean" | "array" | "object" => {
                        schema.insert("type".to_string(), Value::String(s.to_string()));
                    }
                    "file" => {
                        schema.insert("type".to_string(), Value::String("string".to_string()));
                        schema.insert("format".to_string(), Value::String("binary".to_string()));
                    }
                    _ => {
                        schema.insert("type".to_string(), t);
                    }
                }
            } else {
                schema.insert("type".to_string(), t);
            }
        }
        if let Some(f) = p_format {
            schema.insert("format".to_string(), f);
        }
        if let Some(items) = p_items {
            schema.insert("items".to_string(), items);
        }
        if let Some(d) = p_default {
            schema.insert("default".to_string(), d);
        }
        obj.insert("schema".to_string(), Value::Object(schema));
    }
    oas3
}

/// Convert Swagger 2.0 securityDefinitions to OAS3 format.
fn convert_security_definitions(sec_defs: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(obj) = sec_defs.as_object() {
        for (name, def) in obj {
            let new_def = def.clone();
            // Swagger 2.0 securityDefinition.type maps directly to OAS3 type
            // but the structure is slightly different.
            // OAS3 SecurityScheme uses "scheme" for HTTP auth, not in a nested object.
            out.insert(name.clone(), new_def);
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swagger_to_openapi3_basic() {
        let swagger = r##"{
            "swagger": "2.0",
            "info": { "title": "Petstore", "version": "1.0" },
            "host": "petstore.swagger.io",
            "basePath": "/v2",
            "schemes": ["https"],
            "paths": {
                "/pet": {
                    "post": {
                        "tags": ["pet"],
                        "operationId": "addPet",
                        "parameters": [
                            { "name": "body", "in": "body", "schema": { "$ref": "#/definitions/Pet" } }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            },
            "definitions": {
                "Pet": { "type": "object", "properties": { "name": { "type": "string" } } }
            }
        }"##;
        let oas3 = swagger_to_openapi3(swagger).unwrap();
        let parsed: Value = serde_json::from_str(&oas3).unwrap();
        assert_eq!(parsed["openapi"], "3.0.0");
        assert_eq!(
            parsed["servers"][0]["url"],
            "https://petstore.swagger.io/v2"
        );
        assert!(
            parsed["paths"]["/pet"]["post"].get("requestBody").is_some(),
            "body param should become requestBody: {}",
            oas3
        );
        assert!(
            parsed["components"]["schemas"]["Pet"].is_object(),
            "definitions should become components.schemas"
        );
    }

    #[test]
    fn test_swagger_to_openapi3_no_body_params() {
        let swagger = r#"{
            "swagger": "2.0",
            "info": { "title": "Test", "version": "1.0" },
            "host": "api.example.com",
            "paths": {
                "/pets/{petId}": {
                    "get": {
                        "tags": ["pets"],
                        "operationId": "getPetById",
                        "parameters": [
                            { "name": "petId", "in": "path", "required": true, "type": "integer" }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let oas3 = swagger_to_openapi3(swagger).unwrap();
        let parsed: Value = serde_json::from_str(&oas3).unwrap();
        // Should have servers with default https
        assert_eq!(parsed["servers"][0]["url"], "https://api.example.com");
        // Path param should have schema
        let params = &parsed["paths"]["/pets/{petId}"]["get"]["parameters"];
        assert_eq!(params[0]["name"], "petId");
        assert_eq!(params[0]["schema"]["type"], "integer");
    }

    #[test]
    fn test_swagger_formdata_parameters() {
        let swagger = r#"{
            "swagger": "2.0",
            "info": { "title": "Test", "version": "1.0" },
            "host": "api.example.com",
            "paths": {
                "/upload": {
                    "post": {
                        "tags": ["upload"],
                        "operationId": "uploadFile",
                        "parameters": [
                            { "name": "file", "in": "formData", "type": "file", "required": true }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"#;
        let oas3 = swagger_to_openapi3(swagger).unwrap();
        let parsed: Value = serde_json::from_str(&oas3).unwrap();
        let body = &parsed["paths"]["/upload"]["post"]["requestBody"];
        assert!(body.is_object(), "should have requestBody: {}", oas3);
        let content = &body["content"]["multipart/form-data"];
        assert!(content.is_object(), "should be multipart: {}", oas3);
    }

    #[test]
    fn test_swagger_security_definitions() {
        let swagger = r#"{
            "swagger": "2.0",
            "info": { "title": "Test", "version": "1.0" },
            "host": "api.example.com",
            "securityDefinitions": {
                "api_key": { "type": "apiKey", "in": "header", "name": "X-API-Key" }
            },
            "security": [ { "api_key": [] } ],
            "paths": {}
        }"#;
        let oas3 = swagger_to_openapi3(swagger).unwrap();
        let parsed: Value = serde_json::from_str(&oas3).unwrap();
        assert!(
            parsed["components"]["securitySchemes"]["api_key"].is_object(),
            "securityDefinitions should become securitySchemes: {}",
            oas3
        );
        assert!(
            parsed["security"][0].get("api_key").is_some(),
            "security should be preserved"
        );
    }

    #[test]
    fn test_swagger_rejects_non_swagger() {
        let result = swagger_to_openapi3("{}");
        assert!(result.is_err(), "should reject non-swagger spec");
    }

    #[test]
    fn test_swagger_end_to_end() {
        let swagger = r##"{
            "swagger": "2.0",
            "info": { "title": "Petstore", "version": "1.0" },
            "host": "petstore.swagger.io",
            "basePath": "/v2",
            "paths": {
                "/pet": {
                    "post": {
                        "tags": ["pet"],
                        "operationId": "addPet",
                        "parameters": [
                            { "name": "body", "in": "body", "schema": { "$ref": "#/definitions/Pet" } }
                        ],
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        }"##;
        let oas3 = swagger_to_openapi3(swagger).unwrap();
        let result = super::super::openapi::OpenApiImporter::new()
            .import(&oas3)
            .unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "pet.http");
        assert!(result.files[0].content.contains("POST {{base_url}}/pet"));
    }
}
