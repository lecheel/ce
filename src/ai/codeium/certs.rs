//! TLS certificate handling.
//!
//! Builds a secure `reqwest::Client` that trusts OS-native certificate stores
//! and any custom Codeium/Windsurf certificates found on disk.  The client is
//! shared across all HTTP-using modules (auth, completion engine, etc.).
//!
//! NOTE: We use reqwest's rustls-tls feature which handles WebPKI root
//! loading internally, avoiding the need to manually wire up rustls
//! `RootCertStore` + `OwnedTrustAnchor` APIs that differ across versions.

use anyhow::{Context, Result};
use reqwest::Client;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// CertHandler
// ---------------------------------------------------------------------------

pub struct CertHandler {
    client: Client,
}

impl CertHandler {
    /// Initializes a secure HTTP client using reqwest's built-in rustls
    /// support (which loads WebPKI + OS-native roots automatically),
    /// plus any custom Codeium certs found on disk.
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .use_rustls_tls()
            .build()
            .context("Failed to build secure HTTP client")?;

        Ok(Self { client })
    }

    /// Returns a reference to the secure reqwest Client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Looks for custom Codeium/Windsurf certificates in standard locations.
    ///
    /// NOTE: This is currently informational; reqwest's built-in TLS does not
    /// easily allow injecting extra CA certs after construction.  For
    /// environments that need custom CAs, consider setting
    /// `SSL_CERT_FILE` or adding the cert to the OS trust store.
    pub fn load_custom_codeium_certs() -> Option<Vec<Vec<u8>>> {
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

    /// Returns paths where Codeium certificates might live.
    fn get_cert_search_paths() -> Vec<String> {
        let mut paths = vec![
            "~/.codeium/certificates.pem".to_string(),
            "~/.codeium/windsurf/certificates.pem".to_string(),
        ];

        if cfg!(target_os = "windows") {
            paths.push(r"C:\Program Files\Codeium\certificates.pem".to_string());
            paths.push(r"C:\Users\{USER}\AppData\Local\codeium\certificates.pem".to_string());
        } else if cfg!(target_os = "macos") {
            paths.push("/Library/Application Support/Codeium/certificates.pem".to_string());
        } else {
            paths.push("/etc/codeium/certificates.pem".to_string());
            paths.push("/usr/local/share/codeium/certificates.pem".to_string());
        }

        paths
    }

    /// Finds the Windsurf binary if it exists.
    pub fn find_windsurf_binary() -> Option<PathBuf> {
        let search_paths = if cfg!(target_os = "windows") {
            vec![
                r"C:\Program Files\Codeium\windsurf.exe".to_string(),
                r"~\AppData\Local\Programs\Codeium\windsurf.exe".to_string(),
            ]
        } else {
            vec![
                "~/.codeium/windsurf".to_string(),
                "/usr/local/bin/windsurf".to_string(),
                "/opt/codeium/windsurf".to_string(),
            ]
        };

        for path_str in search_paths {
            let expanded = shellexpand::tilde(&path_str).to_string();
            let path = Path::new(&expanded);
            if path.exists() {
                return Some(path.to_path_buf());
            }
        }
        None
    }
}
