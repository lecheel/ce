//! Codeium language-server process management.
//!
//! Spawns the local Codeium language-server binary, waits for it to allocate
//! a port, and provides an async method to fetch completions over the
//! gRPC-JSON gateway.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

// Hardcoded version string as specified
const VERSION: &str = "1.2.26";

fn get_target_info() -> (&'static str, &'static str) {
    #[cfg(target_os = "linux")]
    {
        #[cfg(target_arch = "aarch64")]
        return (
            "language_server_linux_arm",
            "5170d19f24318b082e366fe4260c6c56b2316bb655a1532f9e47c1a8519e41234",
        );
        #[cfg(not(target_arch = "aarch64"))]
        return (
            "language_server_linux_x64",
            "49f056d85774d39e487f63cbc17e220b37637d7a15a81e9f4a76d4e1a859a123",
        );
    }

    #[cfg(target_os = "macos")]
    {
        #[cfg(target_arch = "aarch64")]
        return (
            "language_server_macos_arm",
            "7340432cad6294339d3586ceab314da0915e05495623bffecfb1daa2c33f0f8a",
        );
        #[cfg(not(target_arch = "aarch64"))]
        return (
            "language_server_macos_x64",
            "2c8faaa7cd9da2b6d90b289b095c31d38b620a8473dd2913d6453200594a5b2b",
        );
    }

    #[cfg(target_os = "windows")]
    {
        return (
            "language_server_windows_x64.exe",
            "02829b8180a77f7b4e5b2b0094170e0326cfcebd4b3a80bad2618eef8e6ca0f5",
        );
    }
}

// ---------------------------------------------------------------------------
// CodeiumServer
// ---------------------------------------------------------------------------

pub struct CodeiumServer {
    _process: Child,
    port: u16,
    client: reqwest::Client,
    api_key: String,
    session_id: String,
    request_counter: std::sync::atomic::AtomicU64,
}

impl CodeiumServer {
    /// Locate the language_server binary on disk.
    pub fn find_binary() -> Result<PathBuf> {
        let (binary_name, sha256) = get_target_info();
        log::debug!(
            "Scanning local file directories to find binary: {} with spec hash {}",
            binary_name,
            sha256
        );

        if let Some(home) = dirs::home_dir() {
            let cli_bin = home.join(".codeium").join("bin");

            // 1. Check specification path
            let spec_path = cli_bin
                .join(sha256)
                .join(format!("{}.download", binary_name));
            if spec_path.exists() {
                return Ok(spec_path);
            }

            // 2. Check general fallback path
            let standard_fallback = cli_bin.join(binary_name);
            if standard_fallback.exists() {
                return Ok(standard_fallback);
            }

            // 3. Check Neovim codeium cache (versioned dirs)
            let nvim_bin = home.join(".cache").join("nvim").join("codeium").join("bin");
            if nvim_bin.exists() {
                if let Ok(entries) = fs::read_dir(&nvim_bin) {
                    let mut versions: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                    versions.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
                    for entry in versions {
                        let path = entry.path();
                        if path.is_dir() {
                            let bin_path = path.join(binary_name);
                            if bin_path.exists() {
                                return Ok(bin_path);
                            }
                        }
                    }
                }
            }
        }

        anyhow::bail!("Language server binary not found")
    }

    /// Automatically downloads and unzips the correct binary.
    pub fn download_binary() -> Result<PathBuf> {
        let (binary_name, sha256) = get_target_info();
        let home = dirs::home_dir().context("Could not find user home directory")?;
        let target_dir = home.join(".codeium").join("bin").join(sha256);
        fs::create_dir_all(&target_dir).context("Failed to create bin path")?;

        let url = format!(
            "https://github.com/Exafunction/codeium/releases/download/language-server-v{}/{}.gz",
            VERSION, binary_name
        );

        let final_path = target_dir.join(format!("{}.download", binary_name));

        log::debug!("Starting automated executable setup workflow.");
        log::debug!("URL Source: {}", url);
        log::debug!("File Target: {:?}", final_path);

        println!("Downloading Codeium language server...");
        println!("   From: {}", url);
        println!("   To: {:?}", final_path);

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()?;

        let response = client
            .get(&url)
            .send()
            .context("Failed to download language server")?;
        if !response.status().is_success() {
            anyhow::bail!("Download failed with status code: {}", response.status());
        }

        let bytes = response
            .bytes()
            .context("Failed to read downloaded bytes")?;

        println!("Decompressing language server gzip archive...");

        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decoder = GzDecoder::new(&bytes[..]);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .context("Failed to unzip binary")?;

        // Verify SHA-256
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&decompressed);
        let hash_result = hasher.finalize();
        let calculated_sha = format!("{:x}", hash_result);

        if calculated_sha != sha256 {
            log::debug!(
                "SHA-256 Checksum MISMATCH! Expected: {}, Got: {}",
                sha256,
                calculated_sha
            );
        } else {
            log::debug!("SHA-256 Checksum match confirmed: {}", calculated_sha);
        }

        fs::write(&final_path, decompressed).context("Failed to write executable to disk")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&final_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&final_path, perms)?;
        }

        println!("Language server downloaded successfully!");
        Ok(final_path)
    }

    /// Spawn the language server and return a handle.
    pub async fn new(api_key: String) -> Result<Self> {
        let binary_path = match Self::find_binary() {
            Ok(path) => path,
            Err(_) => {
                log::debug!(
                    "Primary lookup did not locate file. Triggering fresh download workflow."
                );
                Self::download_binary()?
            }
        };

        let home = dirs::home_dir().context("No home directory found")?;
        let database_dir = home.join(".codeium").join("database");
        fs::create_dir_all(&database_dir)?;

        let manager_dir =
            std::env::temp_dir().join(format!("codeium_editor_{}", std::process::id()));
        let _ = fs::remove_dir_all(&manager_dir);
        fs::create_dir_all(&manager_dir)?;

        let child = Command::new(&binary_path)
            .arg("--api_server_url")
            .arg("https://server.codeium.com")
            .arg("--manager_dir")
            .arg(&manager_dir)
            .arg("--database_dir")
            .arg(&database_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to spawn language server")?;

        log::debug!(
            "Process child spawned with ID: {}. Polling local manager dir for random port allocation...",
            child.id()
        );

        let port = Self::wait_for_port_file_async(&manager_dir, Duration::from_secs(60)).await?;
        log::debug!(
            "Language server verified listening on local allocated port: {}",
            port
        );
        tokio::time::sleep(Duration::from_millis(500)).await;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        Ok(Self {
            _process: child,
            port,
            client,
            api_key,
            session_id: uuid::Uuid::new_v4().to_string(),
            request_counter: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Public accessor for the allocated port.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Wait until the language server writes its port file inside the manager directory.
    async fn wait_for_port_file_async(dir: &PathBuf, timeout: Duration) -> Result<u16> {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if let Ok(port) = name.parse::<u16>() {
                                return Ok(port);
                            }
                        }
                        if let Ok(content) = fs::read_to_string(&path) {
                            if let Ok(port) = content.trim().parse::<u16>() {
                                return Ok(port);
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        anyhow::bail!("Timeout waiting for language server port in {:?}", dir)
    }

    /// Fetch completion items from the local language-server gRPC-JSON endpoint.
    pub async fn fetch_completion_items(
        &self,
        full_text: &str,
        cursor_offset: usize,
        language: &str,
    ) -> Result<Vec<String>> {
        let url = format!(
            "http://127.0.0.1:{}/exa.language_server_pb.LanguageServerService/GetCompletions",
            self.port
        );

        let req_id = self
            .request_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let body = serde_json::json!({
            "metadata": {
                "api_key":           self.api_key,
                "ide_name":          "vscode",
                "ide_version":       "1.88.0",
                "extension_version": "1.46.3",
                "request_id":        req_id,
                "session_id":        self.session_id,
                "locale":            "en-US"
            },
            "document": {
                "text":            full_text,
                "cursor_offset":   cursor_offset,
                "editor_language": language,
                "language":        Self::language_to_id(language),
                "line_ending":     if full_text.contains("\r\n") { "\r\n" } else { "\n" },
            },
            "editor_options": {
                "tab_size":      4,
                "insert_spaces": true
            }
        });

        log::debug!(
            "Client async outbound request. Endpoint: {}, Request ID: {}",
            url,
            req_id
        );

        let response = self
            .client
            .post(&url)
            .header("Connect-Protocol-Version", "1")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send GetCompletions request")?;

        let status = response.status();
        let resp_text = response.text().await.unwrap_or_default();

        log::debug!("Client async inbound response status: {:?}", status);

        if !status.is_success() {
            log::debug!(
                "Outbound async exchange failed: HTTP {} | Data: {}",
                status,
                resp_text
            );
            anyhow::bail!(
                "Language server HTTP {}: {}",
                status,
                resp_text.chars().take(200).collect::<String>()
            );
        }

        let resp_json: serde_json::Value = serde_json::from_str(&resp_text)?;

        let mut results = Vec::new();
        if let Some(items) = resp_json.get("completionItems").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(text) = item
                    .get("completion")
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.as_str())
                {
                    if !text.is_empty() {
                        results.push(text.to_string());
                    }
                }
            }
        }

        log::debug!(
            "Async exchanged data parsed. Completions count: {}",
            results.len()
        );
        Ok(results)
    }

    fn language_to_id(language: &str) -> u32 {
        match language {
            "c" => 1,
            "cpp" => 4,
            "css" => 6,
            "go" => 9,
            "html" => 14,
            "java" => 16,
            "javascript" => 17,
            "json" => 19,
            "python" => 33,
            "ruby" => 35,
            "rust" => 36,
            "typescript" => 45,
            "php" => 29,
            "sh" | "bash" => 40,
            "sql" => 41,
            "yaml" | "yml" => 50,
            "markdown" | "md" => 25,
            "lua" => 22,
            "swift" => 46,
            "kotlin" => 20,
            _ => 0,
        }
    }
}

impl Drop for CodeiumServer {
    fn drop(&mut self) {
        let manager_dir =
            std::env::temp_dir().join(format!("codeium_editor_{}", std::process::id()));
        let _ = fs::remove_dir_all(&manager_dir);
    }
}
