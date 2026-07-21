use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A cookie extracted from an HTTP Set-Cookie header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<String>,
    pub http_only: bool,
    pub secure: bool,
}

/// Protocol-agnostic response envelope.
///
/// - HTTP: status/status_text/headers/cookies are populated, metadata has method/redirect_count.
/// - Redis: status=0, status_text=result summary, headers/cookies empty, metadata has command/type.
/// - Future protocols fill what's relevant; Body + Verbose tabs always work from these fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Protocol name, lowercase: "http", "redis", etc.
    pub protocol: String,
    /// HTTP status code; 0 for protocols without status codes.
    pub status: u16,
    /// Human-readable status: "200 OK", "PONG", "3 rows returned".
    pub status_text: String,
    /// Execution latency in milliseconds, measured inside the executor.
    pub latency_ms: u64,
    /// Request URL for HTTP; connection string for DB/Redis.
    pub url: String,
    /// Content-Type for syntax highlighting in the Body tab.
    /// HTTP: from response header. Other protocols: "text/plain".
    pub content_type: String,
    /// Ordered, duplicate-preserving header pairs.
    /// HTTP: actual response headers (Set-Cookie may appear multiple times).
    /// Other protocols: may be empty or carry protocol-specific metadata as pseudo-headers.
    pub headers: Vec<(String, String)>,
    /// Main response content — always present, rendered in the Body tab.
    pub body: String,
    /// Extracted cookies (HTTP only). Empty for non-HTTP protocols.
    pub cookies: Vec<Cookie>,
    /// Protocol-specific extra fields (e.g., Redis "command", "type"; HTTP "method").
    pub metadata: HashMap<String, String>,
}

impl Response {
    /// Find the first header value for a given key (case-insensitive).
    pub fn header(&self, key: &str) -> Option<&str> {
        let key_lower = key.to_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == key_lower)
            .map(|(_, v)| v.as_str())
    }

    /// All values for a given header key (case-insensitive). Useful for Set-Cookie.
    pub fn header_all(&self, key: &str) -> Vec<&str> {
        let key_lower = key.to_lowercase();
        self.headers
            .iter()
            .filter(|(k, _)| k.to_lowercase() == key_lower)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}
