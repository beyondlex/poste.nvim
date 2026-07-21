use crate::response::Cookie;
use std::path::{Path, PathBuf};

/// Cookie jar backed by a Netscape-format file (curl-compatible).
///
/// Cookies are managed by curl via `-b` (read) and `-c` (write) flags,
/// so domain/path matching is handled natively by curl.
///
/// Storage: `~/.cache/poste/cookies.txt`
#[derive(Debug, Clone)]
pub struct CookieJar {
    path: PathBuf,
}

impl CookieJar {
    /// Load or create a cookie jar. Uses standard cache directory.
    /// Ensures the parent directory and cookie file exist so curl doesn't warn.
    pub fn load(_env: &str) -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("poste");
        let _ = std::fs::create_dir_all(&cache_dir);
        let path = cache_dir.join("cookies.txt");
        // Create the file if it doesn't exist — curl warns on missing cookie files
        if !path.exists() {
            let _ = std::fs::write(&path, "# Netscape HTTP Cookie File\n");
        }
        CookieJar { path }
    }

    /// Path to the cookie file (for curl's -b/-c flags).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read cookies from the jar file, returning all cookies.
    /// Used to populate the `cookies` field in Response.
    pub fn read_all(&self) -> Vec<Cookie> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut cookies = Vec::new();
        for line in content.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 7 {
                cookies.push(Cookie {
                    domain: parts[0].to_string(),
                    path: parts[2].to_string(),
                    name: parts[5].to_string(),
                    value: parts[6].to_string(),
                    expires: if parts[4] == "0" {
                        None
                    } else {
                        Some(parts[4].to_string())
                    },
                    http_only: parts[1] == "TRUE",
                    secure: parts[3] == "TRUE",
                });
            }
        }
        cookies
    }

    /// No-op save — curl writes the cookie file directly via `-c`.
    pub fn save(&self) -> anyhow::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}
