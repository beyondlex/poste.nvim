//! MIME type helpers and filename sanitization for HTTP response downloads.
//!
//! Extracted from the monolithic executor.rs to isolate these pure
//! transformation functions.

/// Strip parameters (charset, boundary, etc.) from a Content-Type value,
/// returning just the normalized MIME type.
pub fn normalize_mime_type(content_type: &str) -> String {
    content_type
        .to_lowercase()
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_string()
}

/// Map a Content-Type value to a common file extension (with leading dot).
///
/// Returns `None` for unknown or text-based types.
pub fn mime_to_extension(content_type: &str) -> Option<&'static str> {
    let mime = normalize_mime_type(content_type);
    match mime.as_str() {
        // Images
        "image/jpeg" => Some(".jpg"),
        "image/png" => Some(".png"),
        "image/gif" => Some(".gif"),
        "image/webp" => Some(".webp"),
        "image/svg+xml" => Some(".svg"),
        "image/bmp" => Some(".bmp"),
        "image/tiff" => Some(".tiff"),
        "image/x-icon" => Some(".ico"),
        "image/avif" => Some(".avif"),
        // Audio
        "audio/mpeg" => Some(".mp3"),
        "audio/wav" => Some(".wav"),
        "audio/ogg" => Some(".ogg"),
        "audio/flac" => Some(".flac"),
        "audio/aac" => Some(".aac"),
        "audio/mp4" => Some(".m4a"),
        "audio/webm" => Some(".webm"),
        // Video
        "video/mp4" => Some(".mp4"),
        "video/webm" => Some(".webm"),
        "video/ogg" => Some(".ogv"),
        "video/x-msvideo" => Some(".avi"),
        "video/quicktime" => Some(".mov"),
        "video/x-matroska" => Some(".mkv"),
        // Application
        "application/pdf" => Some(".pdf"),
        "application/zip" => Some(".zip"),
        "application/gzip" => Some(".gz"),
        "application/x-tar" => Some(".tar"),
        "application/x-bzip2" => Some(".bz2"),
        "application/x-7z-compressed" => Some(".7z"),
        "application/x-rar-compressed" => Some(".rar"),
        "application/java-archive" => Some(".jar"),
        "application/octet-stream" => Some(".bin"),
        "application/wasm" => Some(".wasm"),
        "application/x-protobuf" => Some(".pb"),
        "application/msgpack" => Some(".msgpack"),
        "application/cbor" => Some(".cbor"),
        // Office documents
        "application/vnd.ms-excel" => Some(".xls"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some(".xlsx"),
        "application/vnd.ms-powerpoint" => Some(".ppt"),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            Some(".pptx")
        }
        "application/msword" => Some(".doc"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some(".docx"),
        _ => None,
    }
}

/// Detect whether a Content-Type indicates binary data that should not be
/// rendered as text in the response body/verbose tabs.
///
/// Matches common binary MIME types: images, audio, video, archives,
/// office documents, protobuf, etc.
pub fn is_binary_content_type(content_type: &str) -> bool {
    let mime = normalize_mime_type(content_type);

    // Image, audio, video
    if mime.starts_with("image/") || mime.starts_with("audio/") || mime.starts_with("video/") {
        return true;
    }

    // Known binary application types
    matches!(
        mime.as_str(),
        "application/octet-stream"
            | "application/pdf"
            | "application/zip"
            | "application/gzip"
            | "application/x-tar"
            | "application/x-bzip2"
            | "application/x-7z-compressed"
            | "application/x-rar-compressed"
            | "application/java-archive"
            | "application/vnd.ms-excel"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.ms-powerpoint"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/msword"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/x-protobuf"
            | "application/msgpack"
            | "application/cbor"
            | "application/wasm"
    )
}

/// Extract the filename from a Content-Disposition header value.
///
/// Supports formats:
///   `attachment; filename="考勤统计.xls"`
///   `attachment; filename=report.pdf`
///   `inline; filename*=UTF-8''encoded%20name.pdf` (RFC 5987 — returns the
///     percent-encoded string as-is; callers may want to decode it further)
///
/// Returns `None` if no filename parameter is present.
pub fn parse_filename_from_disposition(header_value: &str) -> Option<String> {
    // Look for `filename*=charset'lang'value` (RFC 5987) first — it takes
    // precedence.  We return the raw percent-encoded value so that callers
    // get a valid filename (percent-encoded bytes are safe on disk).
    if let Some(start) = header_value.find("filename*=") {
        let rest = &header_value[start + 10..];
        // After `filename*=`, skip charset'lang' → find the third '.
        let mut quote_count = 0;
        let mut value_start = 0;
        for (i, ch) in rest.char_indices() {
            if ch == '\'' {
                quote_count += 1;
                if quote_count == 3 {
                    value_start = i + 1;
                    break;
                }
            }
        }
        if quote_count == 3 {
            let raw: String = rest[value_start..]
                .trim()
                .trim_matches('"')
                .chars()
                .take_while(|&c| c != ';' && c != ' ')
                .collect();
            if !raw.is_empty() {
                return Some(sanitize_filename(&raw));
            }
        }
    }

    // Look for `filename="value"` or `filename=value`
    if let Some(start) = header_value.find("filename=") {
        let rest = &header_value[start + 9..];
        if let Some(stripped) = rest.strip_prefix('"') {
            // Quoted: filename="value"
            let end = stripped.find('"').map(|i| i + 1).unwrap_or(rest.len());
            let name = &rest[1..end];
            if !name.is_empty() {
                return Some(sanitize_filename(name));
            }
        } else if let Some(stripped) = rest.strip_prefix('\'') {
            // Single-quoted: filename='value'
            let end = stripped.find('\'').map(|i| i + 1).unwrap_or(rest.len());
            let name = &rest[1..end];
            if !name.is_empty() {
                return Some(sanitize_filename(name));
            }
        } else {
            // Unquoted: filename=value  (value ends at ; or whitespace or end)
            let name: String = rest
                .trim()
                .chars()
                .take_while(|&c| c != ';' && c != ' ')
                .collect();
            if !name.is_empty() {
                return Some(sanitize_filename(&name));
            }
        }
    }

    None
}

/// Sanitize a filename for safe use on disk: strip directory separators,
/// null bytes, colons (Windows), and ".." path traversal sequences.
///
/// After sanitization the result is a flat filename with no path components,
/// safe to join with any directory.
pub fn sanitize_filename(name: &str) -> String {
    // First pass: strip dangerous characters
    let sanitized: String = name
        .chars()
        .filter(|&c| c != '/' && c != '\\' && c != '\0' && c != ':')
        .collect();
    // Second pass: remove ".." sequences to prevent path traversal
    let without_dots = sanitized.replace("..", "");
    let trimmed = without_dots.trim().to_string();
    // Avoid empty or dot-only names after sanitization
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "downloaded_file".to_string()
    } else {
        trimmed
    }
}

/// Given a directory and filename, return a path that does not conflict with
/// any existing file.  If the base path already exists, append `(1)`, `(2)`,
/// etc. before the extension (e.g. `report(1).xls`, `report(2).xls`).
///
/// Falls back to the base path if it does not exist, or if we exhaust the
/// range 1..1000 (unlikely, but safe).
pub fn resolve_path_with_conflict(dir: &str, filename: &str) -> std::path::PathBuf {
    let base = std::path::Path::new(dir).join(filename);
    if !base.exists() {
        return base;
    }

    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{}", s))
        .unwrap_or_default();

    for i in 1..1000 {
        // see constants.lua MAX_CONFLICT_SUFFIX
        let candidate = if ext.is_empty() {
            format!("{}({})", stem, i)
        } else {
            format!("{}({}).{}", stem, i, ext.trim_start_matches('.'))
        };
        let path = std::path::Path::new(dir).join(&candidate);
        if !path.exists() {
            return path;
        }
    }

    // Give up and return the original path (will overwrite)
    base
}

/// Trim trailing whitespace and control bytes (space, tab, \r, \n) from a byte
/// slice, returning the trimmed sub-slice.
pub fn trim_end_bytes(mut data: &[u8]) -> &[u8] {
    while let Some(last) = data.last() {
        if last.is_ascii_whitespace() || *last == 0 {
            data = &data[..data.len() - 1];
        } else {
            break;
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // sanitize_filename
    // ---------------------------------------------------------------------------

    #[test]
    fn test_sanitize_filename_normal() {
        assert_eq!(sanitize_filename("report.pdf"), "report.pdf");
    }

    #[test]
    fn test_sanitize_filename_strips_slashes() {
        assert_eq!(sanitize_filename("foo/bar.txt"), "foobar.txt");
    }

    #[test]
    fn test_sanitize_filename_strips_backslashes() {
        assert_eq!(sanitize_filename("foo\\bar.txt"), "foobar.txt");
    }

    #[test]
    fn test_sanitize_filename_prevents_path_traversal() {
        let result = sanitize_filename("../../etc/passwd");
        assert!(!result.contains(".."));
        assert!(!result.contains('/'));

        let result = sanitize_filename("..\\..\\windows\\system32");
        assert!(!result.contains(".."));
        assert!(!result.contains('\\'));
    }

    #[test]
    fn test_sanitize_filename_strips_null_bytes() {
        assert_eq!(sanitize_filename("test\0.txt"), "test.txt");
    }

    #[test]
    fn test_sanitize_filename_strips_colons() {
        assert_eq!(sanitize_filename("file:name.txt"), "filename.txt");
    }

    #[test]
    fn test_sanitize_filename_empty_after_sanitization() {
        assert_eq!(sanitize_filename(".."), "downloaded_file");
        assert_eq!(sanitize_filename("."), "downloaded_file");
        assert_eq!(sanitize_filename(""), "downloaded_file");
    }

    #[test]
    fn test_sanitize_filename_trims_whitespace() {
        assert_eq!(sanitize_filename("  test.txt  "), "test.txt");
    }

    // ---------------------------------------------------------------------------
    // resolve_path_with_conflict
    // ---------------------------------------------------------------------------

    #[test]
    fn test_resolve_path_stays_in_target_dir() {
        let tmp = std::env::temp_dir();
        let path = resolve_path_with_conflict(tmp.to_str().unwrap(), "/etc/passwd");
        assert!(
            path.starts_with(&tmp),
            "path {:?} should start with {:?}",
            path,
            tmp
        );
    }

    #[test]
    fn test_resolve_path_no_conflict() {
        let tmp = std::env::temp_dir();
        let name = format!("poste_test_unique_{}", std::process::id());
        let path = resolve_path_with_conflict(tmp.to_str().unwrap(), &name);
        assert_eq!(path, tmp.join(&name));
        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_resolve_path_with_conflict_appends_suffix() {
        let tmp = std::env::temp_dir();
        let name = format!("poste_test_conflict_{}", std::process::id());
        let path = tmp.join(&name);
        // Create the file so there's a conflict
        let _ = std::fs::write(&path, "existing");
        let resolved = resolve_path_with_conflict(tmp.to_str().unwrap(), &name);
        assert_ne!(resolved, path);
        assert!(resolved.starts_with(&tmp));
        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&resolved);
    }

    // ---------------------------------------------------------------------------
    // is_binary_content_type
    // ---------------------------------------------------------------------------

    #[test]
    fn test_is_binary_content_type_images() {
        assert!(is_binary_content_type("image/png"));
        assert!(is_binary_content_type("image/jpeg"));
        assert!(is_binary_content_type("image/gif"));
    }

    #[test]
    fn test_is_binary_content_type_audio_video() {
        assert!(is_binary_content_type("audio/mpeg"));
        assert!(is_binary_content_type("video/mp4"));
    }

    #[test]
    fn test_is_binary_content_type_application_types() {
        assert!(is_binary_content_type("application/pdf"));
        assert!(is_binary_content_type("application/zip"));
        assert!(is_binary_content_type("application/octet-stream"));
    }

    #[test]
    fn test_is_binary_content_type_text() {
        assert!(!is_binary_content_type("text/plain"));
        assert!(!is_binary_content_type("text/html"));
        assert!(!is_binary_content_type("application/json"));
    }

    #[test]
    fn test_is_binary_content_type_strips_parameters() {
        assert!(is_binary_content_type("image/png; charset=utf-8"));
    }

    // ---------------------------------------------------------------------------
    // mime_to_extension
    // ---------------------------------------------------------------------------

    #[test]
    fn test_mime_to_extension_images() {
        assert_eq!(mime_to_extension("image/jpeg"), Some(".jpg"));
        assert_eq!(mime_to_extension("image/png"), Some(".png"));
        assert_eq!(mime_to_extension("image/gif"), Some(".gif"));
        assert_eq!(mime_to_extension("image/svg+xml"), Some(".svg"));
        assert_eq!(mime_to_extension("image/webp"), Some(".webp"));
        assert_eq!(mime_to_extension("image/bmp"), Some(".bmp"));
        assert_eq!(mime_to_extension("image/tiff"), Some(".tiff"));
        assert_eq!(mime_to_extension("image/x-icon"), Some(".ico"));
    }

    #[test]
    fn test_mime_to_extension_audio_video() {
        assert_eq!(mime_to_extension("audio/mpeg"), Some(".mp3"));
        assert_eq!(mime_to_extension("audio/wav"), Some(".wav"));
        assert_eq!(mime_to_extension("video/mp4"), Some(".mp4"));
        assert_eq!(mime_to_extension("video/webm"), Some(".webm"));
    }

    #[test]
    fn test_mime_to_extension_application() {
        assert_eq!(mime_to_extension("application/pdf"), Some(".pdf"));
        assert_eq!(mime_to_extension("application/zip"), Some(".zip"));
        assert_eq!(mime_to_extension("application/octet-stream"), Some(".bin"));
    }

    #[test]
    fn test_mime_to_extension_text_returns_none() {
        assert_eq!(mime_to_extension("text/plain"), None);
        assert_eq!(mime_to_extension("text/html"), None);
        assert_eq!(mime_to_extension("application/json"), None);
    }

    #[test]
    fn test_mime_to_extension_strips_parameters() {
        assert_eq!(
            mime_to_extension("image/svg+xml; charset=utf-8"),
            Some(".svg")
        );
    }

    #[test]
    fn test_mime_to_extension_office() {
        assert_eq!(
            mime_to_extension("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
            Some(".xlsx")
        );
        assert_eq!(mime_to_extension("application/msword"), Some(".doc"));
        assert_eq!(
            mime_to_extension(
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            ),
            Some(".pptx")
        );
    }

    // ---------------------------------------------------------------------------
    // parse_filename_from_disposition
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_filename_from_disposition_quoted() {
        let result = parse_filename_from_disposition(r#"attachment; filename="report.pdf""#);
        assert_eq!(result, Some("report.pdf".to_string()));
    }

    #[test]
    fn test_parse_filename_from_disposition_unquoted() {
        let result = parse_filename_from_disposition("attachment; filename=report.pdf");
        assert_eq!(result, Some("report.pdf".to_string()));
    }

    #[test]
    fn test_parse_filename_from_disposition_no_filename() {
        let result = parse_filename_from_disposition("attachment");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_filename_from_disposition_sanitizes() {
        let result = parse_filename_from_disposition(r#"attachment; filename="../../etc/passwd""#);
        let result = result.unwrap();
        assert!(!result.contains(".."));
        assert!(!result.contains('/'));
    }
}
