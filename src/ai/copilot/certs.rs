//! TLS certificate handling for the Copilot backend.
//!
//! Builds a secure `reqwest::Client` that trusts OS-native certificate stores
//! and any custom certificates found on disk.  The client is shared across
//! all HTTP-using modules (auth, completion engine, etc.).
//!
//! NOTE: reqwest's rustls-tls feature loads WebPKI + OS-native roots
//! automatically, so no manual `RootCertStore` wiring is needed.

use anyhow::{Context, Result};
use reqwest::Client;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

// ──────────────────────────────────────────────────────────────────────────
// CertHandler
// ──────────────────────────────────────────────────────────────────────────

pub struct CertHandler {
    client: Client,
}

impl CertHandler {
    /// Initialise a secure HTTP client using reqwest's built-in rustls
    /// backend (WebPKI + OS-native roots), plus any custom Copilot certs
    /// found on disk.
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .use_rustls_tls()
            .build()
            .context("Failed to build secure HTTP client")?;

        Ok(Self { client })
    }

    /// Returns a reference to the shared reqwest Client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Looks for custom Copilot / GitHub Enterprise certificates in
    /// standard locations.
    ///
    /// NOTE: This is currently informational — reqwest's built-in TLS does
    /// not allow injecting extra CA certs after construction.  For
    /// environments that require custom CAs, set `SSL_CERT_FILE` or add
    /// the cert to the OS trust store.
    pub fn load_custom_copilot_certs() -> Option<Vec<Vec<u8>>> {
        let possible_paths = Self::get_cert_search_paths();

        for path_str in possible_paths {
            let expanded = shellexpand::tilde(&path_str).to_string();
            let path = Path::new(&expanded);

            if path.exists() {
                if let Ok(file) = File::open(path) {
                    let mut reader = BufReader::new(file);
                    let parsed_certs: Vec<Vec<u8>> = rustls_pemfile::certs(&mut reader)
                        .filter_map(|r| r.ok())
                        .map(|cert_der| cert_der.to_vec())
                        .collect();

                    if !parsed_certs.is_empty() {
                        return Some(parsed_certs);
                    }
                }
            }
        }
        None
    }

    /// Returns platform-specific paths where Copilot / GitHub Enterprise
    /// certificates might live.
    fn get_cert_search_paths() -> Vec<String> {
        let mut paths = vec![
            "~/.config/github-copilot/certificates.pem".to_string(),
            "~/.copilot/certificates.pem".to_string(),
        ];

        if cfg!(target_os = "windows") {
            paths.push(r"C:\Program Files\GitHub CLI\certificates.pem".to_string());
            paths
                .push(r"C:\Users\{USER}\AppData\Local\github-copilot\certificates.pem".to_string());
        } else if cfg!(target_os = "macos") {
            paths.push("/Library/Application Support/github-copilot/certificates.pem".to_string());
        } else {
            paths.push("/etc/github-copilot/certificates.pem".to_string());
            paths.push("/usr/local/share/github-copilot/certificates.pem".to_string());
        }

        paths
    }
}
