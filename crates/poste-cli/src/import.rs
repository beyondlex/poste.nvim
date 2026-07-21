use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
pub enum ImportSource {
    /// Import from OpenAPI 3.x spec (.json or .yaml)
    Openapi {
        /// Path to OpenAPI spec file
        file: String,
        /// Output directory for generated .http files
        #[arg(short, long)]
        out: String,
    },
    /// Import from Swagger 2.0 spec (.json or .yaml)
    Swagger {
        /// Path to Swagger spec file
        file: String,
        /// Output directory for generated .http files
        #[arg(short, long)]
        out: String,
    },
    /// Import from Postman Collection v2.1 export (.json)
    Postman {
        /// Path to Postman collection file
        file: String,
        /// Output directory for generated .http files
        #[arg(short, long)]
        out: String,
    },
}

pub fn execute(source: ImportSource) -> Result<()> {
    let (file, out_dir, importer): (String, String, Box<dyn poste_core::import::SpecImporter>) =
        match source {
            ImportSource::Openapi { file, out } => (
                file,
                out,
                Box::new(poste_core::import::openapi::OpenApiImporter::new()),
            ),
            ImportSource::Swagger { file, out } => (
                file,
                out,
                Box::new(poste_core::import::swagger::SwaggerImporter),
            ),
            ImportSource::Postman { file, out } => (
                file,
                out,
                Box::new(poste_core::import::postman::PostmanImporter),
            ),
        };

    let spec_content = std::fs::read_to_string(&file)
        .map_err(|e| anyhow::anyhow!("Cannot read spec file '{}': {}", file, e))?;
    let result = importer.import(&spec_content)?;

    let out_path = std::path::Path::new(&out_dir);
    std::fs::create_dir_all(out_path)
        .map_err(|e| anyhow::anyhow!("Cannot create output directory '{}': {}", out_dir, e))?;

    for http_file in &result.files {
        let full_path = out_path.join(&http_file.path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!("Cannot create subdirectory '{}': {}", parent.display(), e)
            })?;
        }
        if full_path.exists() {
            eprintln!(
                "[poste import] warning: overwriting {}",
                full_path.display()
            );
        }
        std::fs::write(&full_path, &http_file.content)
            .map_err(|e| anyhow::anyhow!("Cannot write '{}': {}", full_path.display(), e))?;
    }

    // Write env.json if there are extracted variables
    if !result.env_vars.is_empty() {
        let env_path = out_path.join("env.json");
        let env_content = serde_json::to_string_pretty(&serde_json::json!({
            "dev": result.env_vars
        }))?;
        std::fs::write(&env_path, &env_content)?;
    }

    println!(
        "OK — imported {} file(s) to {}",
        result.files.len(),
        out_dir
    );
    if !result.env_vars.is_empty() {
        println!("  env.json: {} variable(s)", result.env_vars.len());
    }
    for w in &result.warnings {
        eprintln!("  warning: {}", w);
    }

    Ok(())
}
