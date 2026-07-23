use crate::cookie_jar::CookieJar;
use crate::mime;
use crate::response::Response;
use anyhow::Result;
use poste_core::{Protocol, Request};
use std::collections::HashMap;
use std::time::Instant;

pub struct Executor;

impl Executor {
    pub async fn execute(request: &Request, cookie_jar: Option<&CookieJar>) -> Result<Response> {
        let start = Instant::now();
        let mut response = match request.protocol {
            Protocol::Http => Self::execute_http(request, cookie_jar).await,
            Protocol::Redis => Executor::execute_redis(request).await,
            Protocol::Mysql | Protocol::Postgres | Protocol::Sqlite => {
                crate::sql_executor::execute_sql(request).await
            }
        }?;
        response.latency_ms = start.elapsed().as_millis() as u64;
        Ok(response)
    }

    /// Execute HTTP request via curl subprocess.
    ///
    /// Using curl gives us verbose trace output (TLS, DNS, proxy, HTTP/2) for free,
    /// identical to what kulala.nvim shows in its Verbose tab.
    async fn execute_http(request: &Request, cookie_jar: Option<&CookieJar>) -> Result<Response> {
        let body_bytes = &request.body;

        // Find the header/body boundary in the raw bytes by scanning for
        // \r\n\r\n or \n\n.  The headers portion is always ASCII/UTF-8,
        // but the body may contain binary data (e.g. PNG file upload).
        let boundary = body_bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| (p, 4))
            .or_else(|| {
                body_bytes
                    .windows(2)
                    .position(|w| w == b"\n\n")
                    .map(|p| (p, 2))
            });

        let (head_bytes, body_part) = match boundary {
            Some((pos, sep_len)) => (&body_bytes[..pos], &body_bytes[pos + sep_len..]),
            None => {
                // No blank line → headers only, no body
                (body_bytes.as_ref(), &[][..])
            }
        };

        let head_str = std::str::from_utf8(head_bytes)
            .map_err(|_| anyhow::anyhow!("HTTP request headers contain invalid UTF-8"))?;
        let lines: Vec<&str> = head_str.lines().collect();
        if lines.is_empty() {
            anyhow::bail!("Empty HTTP request");
        }

        let request_line = lines[0].trim();
        let space_pos = request_line
            .find(char::is_whitespace)
            .ok_or_else(|| anyhow::anyhow!("Invalid HTTP request line: {}", request_line))?;

        let method = request_line[..space_pos].to_uppercase();
        let url = request_line[space_pos..].trim_start().to_string();
        // Strip HTTP version suffix (e.g. " HTTP/1.1") from the URL
        let url = url.split_whitespace().next().unwrap_or(&url).to_string();

        let mut req_headers = Vec::new();
        for line in lines.iter().skip(1) {
            if line.trim().is_empty() {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                req_headers.push((key.trim().to_string(), value.trim().to_string()));
            }
        }

        // Trim trailing whitespace/CR/LF from body part
        let body_part = mime::trim_end_bytes(body_part);

        // For application/x-www-form-urlencoded, strip newlines entirely.
        let is_form_urlencoded = req_headers.iter().any(|(k, v)| {
            k.to_lowercase() == "content-type" && v.contains("x-www-form-urlencoded")
        });

        // For multipart/form-data, convert LF → CRLF as required by the MIME
        // standard (RFC 2046).  The .http file uses Unix-style \n, but HTTP
        // boundary delimiters and part headers must use \r\n.
        let is_multipart = req_headers
            .iter()
            .any(|(k, v)| k.to_lowercase() == "content-type" && v.contains("multipart/form-data"));

        let body_to_send: Vec<u8> = if is_form_urlencoded {
            body_part.iter().filter(|&&b| b != b'\n').copied().collect()
        } else if is_multipart {
            // Replace \n (0x0A) with \r\n (0x0D 0x0A) — note this may affect
            // binary \n bytes in non-text file content, which is a known
            // limitation for multipart binary uploads.
            let mut v = Vec::with_capacity(body_part.len() + 16);
            for &b in body_part {
                if b == b'\n' {
                    v.push(b'\r');
                }
                v.push(b);
            }
            v
        } else {
            body_part.to_vec()
        };

        let headers_file = tempfile::NamedTempFile::new()?;
        let headers_path = headers_file.path().to_path_buf();

        // Write the request body to a temp file so curl uses
        // `--data-binary @path` instead of inline data — this avoids
        // "nul byte found in provided data" errors for binary content.
        let body_file = tempfile::NamedTempFile::new()?;
        let body_path = body_file.path().to_path_buf();
        std::fs::write(&body_path, &body_to_send)?;

        let args = Self::build_curl_args(
            &method,
            &url,
            &req_headers,
            &body_path,
            cookie_jar,
            &headers_path,
        );
        let (stdout, stderr, status) = Self::execute_curl(&args).await?;
        let headers_content = std::fs::read_to_string(&headers_path).unwrap_or_default();

        let mut response = parse_curl_response(&headers_content, &stdout, &url)?;

        let cookies = cookie_jar
            .as_ref()
            .map(|j| j.read_all())
            .unwrap_or_default();

        let mut metadata = HashMap::new();

        // If the response is binary content (image, PDF, zip, etc.), save it to
        // /tmp/ and store the file path in metadata instead of mangled UTF-8 text.
        if mime::is_binary_content_type(&response.content_type) && !stdout.is_empty() {
            // Try to extract filename from Content-Disposition header (e.g.,
            // `attachment; filename="考勤统计.xls"`), falling back to a
            // timestamp-based name.
            let disp_header = response
                .headers
                .iter()
                .find(|(k, _)| k.to_lowercase() == "content-disposition")
                .map(|(_, v)| v.as_str());
            let file_name = disp_header
                .and_then(mime::parse_filename_from_disposition)
                .filter(|n: &String| !n.is_empty())
                .unwrap_or_else(|| {
                    format!(
                        "poste_{}_{}",
                        chrono::Local::now().format("%Y%m%d_%H%M%S"),
                        response.status
                    )
                });
            // Append a file extension based on Content-Type when the filename
            // has none yet (common for timestamp fallback or bare filenames).
            let file_name = if !file_name.contains('.') {
                mime::mime_to_extension(&response.content_type)
                    .map(|ext| format!("{file_name}{ext}"))
                    .unwrap_or(file_name)
            } else {
                file_name
            };
            let cache_dir = std::env::var("POSTE_CACHE_DIR").unwrap_or_else(|_| "/tmp".to_string());
            std::fs::create_dir_all(&cache_dir).ok();
            let tmp_path = mime::resolve_path_with_conflict(&cache_dir, &file_name);
            match std::fs::write(&tmp_path, &stdout) {
                Err(e) => {
                    metadata.insert(
                        "file_save_error".to_string(),
                        format!("failed to write to {}: {}", tmp_path.display(), e),
                    );
                }
                Ok(()) => {
                    let file_size = stdout.len();
                    metadata.insert(
                        "file_path".to_string(),
                        tmp_path.to_string_lossy().to_string(),
                    );
                    metadata.insert("file_size".to_string(), file_size.to_string());
                    metadata.insert(
                        "file_content_type".to_string(),
                        response.content_type.clone(),
                    );
                    // Replace mangled body with a summary
                    response.body = format!(
                        "[Binary file saved to: {}]\n[Size: {} bytes]\n[Content-Type: {}]",
                        tmp_path.display(),
                        file_size,
                        response.content_type
                    );
                }
            }
        }

        metadata.insert("method".to_string(), method.clone());
        metadata.insert(
            "request_headers".to_string(),
            req_headers
                .iter()
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        // Display the raw (unresolved) body in the request preview, so file
        // includes like `< ./photo.png` are shown as-is rather than dumping
        // the expanded binary content into the Verbose tab.
        let display_body = if !request.raw_body.trim().is_empty() {
            request.raw_body.clone()
        } else {
            String::from_utf8_lossy(body_part).to_string()
        };
        if !display_body.trim().is_empty() {
            metadata.insert("request_body".to_string(), display_body.clone());
        }
        metadata.insert(
            "timestamp".to_string(),
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        );
        metadata.insert(
            "verbose".to_string(),
            String::from_utf8_lossy(&stderr).to_string(),
        );
        metadata.insert(
            "exit_code".to_string(),
            status.code().unwrap_or(-1).to_string(),
        );

        Ok(Response {
            protocol: "http".to_string(),
            status: response.status,
            status_text: response.status_text,
            latency_ms: 0, // filled by execute()
            url,
            content_type: response.content_type,
            headers: response.headers,
            body: response.body,
            cookies,
            metadata,
        })
    }

    /// Build curl argument list from parsed request components.
    ///
    /// `body_path` is a path to a temp file containing the request body.
    /// Using `--data-binary @path` avoids "nul byte found in provided data"
    /// errors when the body contains binary content (e.g. PNG uploads).
    fn build_curl_args(
        method: &str,
        url: &str,
        req_headers: &[(String, String)],
        body_path: &std::path::Path,
        cookie_jar: Option<&CookieJar>,
        headers_path: &std::path::Path,
    ) -> Vec<String> {
        let mut args = vec![
            "-s".to_string(),
            "-S".to_string(),
            "-v".to_string(),
            "-L".to_string(),
            "--compressed".to_string(),
            "-X".to_string(),
            method.to_string(),
            "-D".to_string(),
            headers_path.to_string_lossy().to_string(),
            "-A".to_string(),
            "poste/0.1.0".to_string(),
        ];

        for (key, value) in req_headers {
            args.push("-H".to_string());
            args.push(format!("{}: {}", key, value));
        }

        if body_path.exists() && body_path.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
            args.push("--data-binary".to_string());
            args.push(format!("@{}", body_path.display()));
        }

        if let Some(jar) = &cookie_jar {
            let path = jar.path().to_string_lossy().to_string();
            args.push("-b".to_string());
            args.push(path.clone());
            args.push("-c".to_string());
            args.push(path);
        }

        args.push(url.to_string());
        args
    }

    /// Execute curl subprocess and return (stdout, stderr, exit_status).
    async fn execute_curl(args: &[String]) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus)> {
        let output = tokio::process::Command::new("curl")
            .args(args)
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to execute curl: {}. Is curl installed?", e))?;
        Ok((output.stdout, output.stderr, output.status))
    }
}

/// Parsed response from curl output.
struct CurlResponse {
    status: u16,
    status_text: String,
    content_type: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Parse response headers from curl's -D file and body from stdout.
///
/// The headers file may contain multiple header blocks (one per redirect hop).
/// We parse the last block, which is the final response.
fn parse_curl_response(
    headers_content: &str,
    body_bytes: &[u8],
    request_url: &str,
) -> Result<CurlResponse> {
    // Split into header blocks separated by blank lines.
    // Take the last non-empty block (final response after redirects).
    let blocks: Vec<&str> = headers_content
        .split("\r\n\r\n")
        .filter(|b| !b.trim().is_empty())
        .collect();

    let last_block = blocks.last().copied().unwrap_or("");

    let mut status: u16 = 0;
    let mut status_text = String::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_type = "text/plain".to_string();

    for line in last_block.lines() {
        let line = line.trim();
        if line.starts_with("HTTP/") {
            // Status line: "HTTP/2 200" or "HTTP/1.1 200 OK"
            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            if parts.len() >= 2 {
                status = parts[1].parse().unwrap_or(0);
                status_text = if parts.len() >= 3 && !parts[2].is_empty() {
                    format!("{} {}", status, parts[2])
                } else {
                    // HTTP/2 has no reason phrase; look up common codes
                    format!("{} {}", status, http_reason(status))
                };
            }
        } else if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            if key.to_lowercase() == "content-type" {
                content_type = value.clone();
            }
            headers.push((key, value));
        }
    }

    // If no status line found (e.g., empty headers), infer from body presence
    if status == 0 && !body_bytes.is_empty() {
        status = 200;
        status_text = "200 OK".to_string();
    }

    let body = String::from_utf8_lossy(body_bytes).to_string();

    // If request_url was empty, try to extract Host header for display
    let _ = request_url; // may use later for URL enrichment

    Ok(CurlResponse {
        status,
        status_text,
        content_type,
        headers,
        body,
    })
}

/// HTTP reason phrases for common status codes (HTTP/2 doesn't include them).
fn http_reason(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// Parse shell-style arguments, handling quotes
pub(crate) fn parse_shell_args(input: &str) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';
    let chars = input.chars().peekable();

    for c in chars {
        match c {
            '"' | '\'' if !in_quotes => {
                in_quotes = true;
                quote_char = c;
            }
            c if c == quote_char && in_quotes => {
                in_quotes = false;
            }
            '`' if !in_quotes => {
                // Skip backticks - they're markdown formatting, not part of the command
                continue;
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if in_quotes {
        anyhow::bail!("Unclosed quote in command: {}", input);
    }

    if !current.is_empty() {
        args.push(current);
    }

    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // parse_curl_response
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_curl_response_simple() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n";
        let body = b"{\"key\": \"value\"}";
        let response = parse_curl_response(headers, body, "http://example.com").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.status_text, "200 OK");
        assert_eq!(response.content_type, "application/json");
        assert_eq!(response.body, "{\"key\": \"value\"}");
    }

    #[test]
    fn test_parse_curl_response_redirect() {
        let headers = "HTTP/1.1 301 Moved Permanently\r\nLocation: /new\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n";
        let body = b"final response";
        let response = parse_curl_response(headers, body, "http://example.com").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.status_text, "200 OK");
    }

    #[test]
    fn test_parse_curl_response_http2() {
        let headers = "HTTP/2 200\r\ncontent-type: application/json\r\n\r\n";
        let body = b"{}";
        let response = parse_curl_response(headers, body, "http://example.com").unwrap();
        assert_eq!(response.status, 200);
        assert!(response.status_text.contains("OK"));
    }

    #[test]
    fn test_parse_curl_response_empty_headers() {
        let body = b"some content";
        let response = parse_curl_response("", body, "http://example.com").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.status_text, "200 OK");
        assert_eq!(response.body, "some content");
    }

    // ---------------------------------------------------------------------------
    // http_reason
    // ---------------------------------------------------------------------------

    #[test]
    fn test_http_reason_common_codes() {
        assert_eq!(http_reason(200), "OK");
        assert_eq!(http_reason(404), "Not Found");
        assert_eq!(http_reason(500), "Internal Server Error");
    }

    #[test]
    fn test_http_reason_unknown_code() {
        assert_eq!(http_reason(999), "");
    }
}
