use super::*;

// ---------------------------------------------------------------------------
// Step 0: Empty spec returns empty result
// ---------------------------------------------------------------------------

#[test]
fn test_empty_openapi_creates_no_files() {
    let spec = r#"{"openapi":"3.0.0","info":{"title":"Empty","version":"1.0"},"paths":{}}"#;
    let importer = openapi::OpenApiImporter::new();
    let result = importer.import(spec).unwrap();
    assert!(
        result.files.is_empty(),
        "empty spec should produce no files"
    );
}

#[test]
fn test_empty_swagger_creates_no_files() {
    let spec = r#"{"swagger":"2.0","info":{"title":"Empty"},"paths":{}}"#;
    let importer = swagger::SwaggerImporter;
    let result = importer.import(spec).unwrap();
    assert!(
        result.files.is_empty(),
        "empty swagger should produce no files"
    );
}

#[test]
fn test_empty_postman_creates_no_files() {
    let spec = r#"{"info":{"name":"Empty","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[]}"#;
    let importer = postman::PostmanImporter;
    let result = importer.import(spec).unwrap();
    assert!(
        result.files.is_empty(),
        "empty collection should produce no files"
    );
}

// ---------------------------------------------------------------------------
// Step 1: ImportResult / HttpFile serialization
// ---------------------------------------------------------------------------

#[test]
fn test_import_result_serialization() {
    let result = ImportResult {
        files: vec![HttpFile {
            path: "test.http".into(),
            content: "### Test\nGET /api\n".into(),
        }],
        env_vars: [("base_url".into(), "https://example.com".into())].into(),
        warnings: vec!["test warning".into()],
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("test.http"));
    assert!(json.contains("GET /api"));
    assert!(json.contains("base_url"));
    assert!(json.contains("test warning"));

    // Round-trip
    let decoded: ImportResult = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.files.len(), 1);
    assert_eq!(decoded.files[0].path, "test.http");
    assert_eq!(
        decoded.env_vars.get("base_url").unwrap(),
        "https://example.com"
    );
}

// ---------------------------------------------------------------------------
// Step 1: HttpFile basic properties
// ---------------------------------------------------------------------------

#[test]
fn test_http_file_construction() {
    let f = HttpFile {
        path: "pets/list.http".into(),
        content: "### List pets\nGET /api/pets\n".into(),
    };
    assert_eq!(f.path, "pets/list.http");
    assert!(f.content.starts_with("### List pets"));
}

// ---------------------------------------------------------------------------
// Step 1: Trait object dispatch
// ---------------------------------------------------------------------------

#[test]
fn test_trait_object_dispatch() {
    let importers: Vec<Box<dyn SpecImporter>> = vec![
        Box::new(openapi::OpenApiImporter::new()),
        Box::new(swagger::SwaggerImporter),
        Box::new(postman::PostmanImporter),
    ];
    for importer in &importers {
        // Use an empty valid spec for OpenAPI, empty JSON for placeholder importers
        let result = importer.import("{}").unwrap_or_else(|_| ImportResult {
            files: vec![],
            env_vars: std::collections::HashMap::new(),
            warnings: vec![],
        });
        assert!(result.files.is_empty());
    }
}
