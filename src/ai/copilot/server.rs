//! GitHub Copilot language-server process management.
//!
//! Spawns the local Copilot agent, performs the LSP handshake over stdio, and
//! provides an async method to fetch completions via the Copilot-specific
//! `getCompletions` JSON-RPC method.
//!
//! **Auto-download:** If the binary is not found on the system, it is
//! automatically downloaded from the official `github/copilot-language-server-release`
//! GitHub releases and installed to `~/.local/share/ce/`.
//!
//! **Auth model:** On startup the agent reads the OAuth token from
//! `~/.config/github-copilot/hosts.json` during `checkStatus`.  If the token
//! is invalid/expired the agent stays unauthenticated — completions are
//! rejected fast.  The only way to re-auth is `:copilot-auth`, which saves a
//! fresh token to `hosts.json` and sends a `Reauth` request so the agent
//! re-reads it.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read as StdRead, Write as StdWrite};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

// ──────────────────────────────────────────────────────────────────────────
// Binary type
// ──────────────────────────────────────────────────────────────────────────

/// Describes which kind of Copilot agent was found on disk.
pub enum CopilotBinary {
    /// Standalone `copilot-language-server` executable (no Node.js needed).
    Executable(PathBuf),
    /// Node.js agent (`agent.js` from a Neovim/Vim/VS Code plugin).
    NodeAgent(PathBuf),
}

// ──────────────────────────────────────────────────────────────────────────
// CopilotServer
// ──────────────────────────────────────────────────────────────────────────

pub struct CopilotServer {
    _process: Child,
    write_tx: mpsc::UnboundedSender<Vec<u8>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    request_id: AtomicU64,
    #[allow(dead_code)]
    oauth_token: String,
    #[allow(dead_code)]
    session_id: String,
}

impl CopilotServer {
    // ══════════════════════════════════════════════════════════════════════
    // Binary discovery
    // ══════════════════════════════════════════════════════════════════════

    pub fn find_binary() -> Result<CopilotBinary> {
        log::debug!("Scanning for the Copilot agent binary...");

        // ── 1. Environment variable override ──────────────────────
        if let Ok(path) = std::env::var("COPILOT_AGENT_PATH") {
            let p = PathBuf::from(&path);
            if p.exists() {
                log::debug!("Found Copilot agent via COPILOT_AGENT_PATH: {:?}", p);
                return if path.ends_with(".js") {
                    Ok(CopilotBinary::NodeAgent(p))
                } else {
                    Ok(CopilotBinary::Executable(p))
                };
            } else {
                log::warn!("COPILOT_AGENT_PATH set to {:?} but file missing", p);
            }
        }

        // ── 2. Standalone executable in $PATH ─────────────────────
        if let Ok(path) = which::which("copilot-language-server") {
            log::debug!("Found copilot-language-server in PATH: {:?}", path);
            return Ok(CopilotBinary::Executable(path));
        }

        // ── 3. Previously auto-installed binary ───────────────────
        if let Ok(path) = Self::installed_binary_path() {
            if path.exists() {
                log::debug!("Found auto-installed Copilot agent at: {:?}", path);
                return Ok(CopilotBinary::Executable(path));
            }
        }

        // ── 4. Previously auto-installed Node.js agent ────────────
        if let Ok(dir) = Self::install_dir() {
            let agent_path = dir.join("copilot-agent.js");
            if agent_path.exists() {
                log::debug!("Found auto-installed agent.js at: {:?}", agent_path);
                return Ok(CopilotBinary::NodeAgent(agent_path));
            }
        }

        // ── 5. Standard standalone locations ──────────────────────
        if let Some(home) = dirs::home_dir() {
            let candidates = vec![
                home.join(".copilot/language-server"),
                home.join(".local/bin/copilot-language-server"),
            ];
            for candidate in &candidates {
                if candidate.exists() {
                    log::debug!("Found standalone agent at: {:?}", candidate);
                    return Ok(CopilotBinary::Executable(candidate.clone()));
                }
            }
        }

        // ── 6. Fallback: bundled agent.js from editor plugins ─────
        let home = dirs::home_dir().context("Could not find home directory")?;
        let node_candidates: Vec<PathBuf> = vec![
            home.join(".local/share/nvim/lazy/copilot.lua/copilot/dist/agent.js"),
            home.join(".local/share/nvim/lazy/copilot.vim/dist/agent.js"),
            home.join(".local/share/nvim/site/pack/github/start/copilot.lua/copilot/dist/agent.js"),
            home.join(".local/share/nvim/site/pack/github/start/copilot.vim/dist/agent.js"),
            home.join(".vim/plugged/copilot.vim/dist/agent.js"),
            home.join(".local/share/nvim/plugged/copilot.vim/dist/agent.js"),
            home.join(".vscode/extensions/github.copilot-*/dist/agent.js"),
        ];

        for candidate in &node_candidates {
            if candidate.exists() {
                log::debug!("Found Node.js Copilot agent at: {:?}", candidate);
                return Ok(CopilotBinary::NodeAgent(candidate.clone()));
            }

            if let Some(parent) = candidate.parent() {
                let parent_str = parent.to_string_lossy();
                if parent_str.contains('*') {
                    if let Ok(entries) = glob::glob(&parent_str) {
                        for entry in entries.flatten() {
                            let file_name = candidate.file_name().unwrap_or_default();
                            let full = entry.join(file_name);
                            if full.exists() {
                                log::debug!("Found agent (glob) at: {:?}", full);
                                return Ok(CopilotBinary::NodeAgent(full));
                            }
                        }
                    }
                }
            }
        }

        bail!("Copilot agent not found. Auto-download will be attempted on startup.")
    }

    fn find_node() -> Result<PathBuf> {
        which::which("node").context(
            "Node.js is required for the Copilot agent.js. \
             Install the standalone copilot-language-server binary instead, \
             or install Node.js (>= 18).",
        )
    }

    // ══════════════════════════════════════════════════════════════════════
    // Auto-download
    // ══════════════════════════════════════════════════════════════════════

    fn install_dir() -> Result<PathBuf> {
        let dir = dirs::data_local_dir().context("Could not find local data directory")?;
        Ok(dir.join("ce"))
    }

    fn installed_binary_path() -> Result<PathBuf> {
        let name = if cfg!(windows) {
            "copilot-language-server.exe"
        } else {
            "copilot-language-server"
        };
        Ok(Self::install_dir()?.join(name))
    }

    fn platform_id() -> String {
        let os = match std::env::consts::OS {
            "linux" => "linux",
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => other,
        };
        format!("{}-{}", os, arch)
    }

    /// Auto-download the Copilot language server from the official
    /// `github/copilot-language-server-release` GitHub releases.
    async fn auto_download_binary() -> Result<CopilotBinary> {
        log::info!("Attempting to auto-download Copilot language server...");

        let client = reqwest::Client::builder()
            .user_agent("ce-editor/0.1.0")
            .build()
            .context("Failed to build HTTP client")?;

        // ── Step 1: Query latest release ──────────────────────────
        let api_url =
            "https://api.github.com/repos/github/copilot-language-server-release/releases/latest";
        log::debug!("Querying: {}", api_url);

        let resp = client
            .get(api_url)
            .send()
            .await
            .context("Failed to query GitHub releases. Check your network connection.")?;

        if !resp.status().is_success() {
            bail!(
                "GitHub API returned {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }

        let json: Value = resp.json().await?;
        let tag = json
            .get("tag_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let assets = json
            .get("assets")
            .and_then(|v| v.as_array())
            .context("No assets in release")?;

        log::info!("Latest release: {} ({} assets)", tag, assets.len());

        // ── Step 2: Find matching ZIP for our platform ────────────
        let platform = Self::platform_id();
        let standalone_prefix = format!("copilot-language-server-{}", platform);

        let (asset_name, download_url) = assets
            .iter()
            .find_map(|asset| {
                let name = asset.get("name").and_then(|n| n.as_str())?;
                if name.starts_with(&standalone_prefix) && name.ends_with(".zip") {
                    let url = asset.get("browser_download_url").and_then(|u| u.as_str())?;
                    Some((name.to_string(), url.to_string()))
                } else {
                    None
                }
            })
            .context(format!(
                "No matching release asset for platform '{}' \
                 (prefix '{}'). Available: {:?}",
                platform,
                standalone_prefix,
                assets
                    .iter()
                    .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
            ))?;

        // ── Step 3: Download the ZIP ─────────────────────────────
        log::info!("Downloading {} ...", asset_name);
        let resp = client.get(&download_url).send().await?;

        if !resp.status().is_success() {
            bail!("Download failed: HTTP {}", resp.status());
        }

        let data = resp.bytes().await?;
        log::info!("Downloaded {} bytes", data.len());

        // ── Step 4: Extract binary from ZIP ───────────────────────
        let reader = std::io::Cursor::new(&data[..]);
        let mut archive = zip::ZipArchive::new(reader).context("Failed to open ZIP archive")?;

        let _bin_name = if cfg!(windows) {
            "copilot-language-server.exe"
        } else {
            "copilot-language-server"
        };

        // Find the binary inside the ZIP (may have a directory prefix)
        let mut binary_data: Option<Vec<u8>> = None;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let name = file.name().to_string();

            // Skip directories
            if file.is_dir() {
                continue;
            }

            let name_lower = name.to_lowercase();

            // Match the binary file
            let is_match = if cfg!(windows) {
                name_lower.ends_with(".exe") && name_lower.contains("copilot-language-server")
            } else {
                name_lower.contains("copilot-language-server")
                    && !name_lower.ends_with(".js")
                    && !name_lower.ends_with(".map")
                    && !name_lower.ends_with(".pdb")
                    && !name_lower.contains("node_modules")
            };

            if is_match {
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                binary_data = Some(buf);
                log::debug!(
                    "Extracted '{}' from ZIP ({} bytes)",
                    name,
                    binary_data.as_ref().unwrap().len()
                );
                break;
            }
        }

        // Fallback: take the first non-directory, non-trivial file
        if binary_data.is_none() {
            for i in 0..archive.len() {
                let mut file = archive.by_index(i)?;
                if file.is_dir() {
                    continue;
                }
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                if buf.len() > 1000 {
                    binary_data = Some(buf);
                    log::debug!(
                        "Fallback: extracted '{}' from ZIP ({} bytes)",
                        file.name(),
                        binary_data.as_ref().unwrap().len()
                    );
                    break;
                }
            }
        }

        let binary_data =
            binary_data.context("No binary found inside the downloaded ZIP archive")?;

        if binary_data.len() < 1000 {
            bail!(
                "Extracted binary is suspiciously small ({} bytes).",
                binary_data.len()
            );
        }

        // ── Step 5: Install to disk ──────────────────────────────
        let install_dir = Self::install_dir()?;
        std::fs::create_dir_all(&install_dir)?;

        let binary_path = Self::installed_binary_path()?;

        // Write to temp file first, then rename (atomic on same fs)
        let temp_path = binary_path.with_extension("downloading");
        {
            let mut f = std::fs::File::create(&temp_path).context("Failed to create temp file")?;
            StdWrite::write_all(&mut f, &binary_data).context("Failed to write binary")?;
            StdWrite::flush(&mut f)?;
        }

        // Make executable (Unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o755))?;
        }

        if binary_path.exists() {
            let _ = std::fs::remove_file(&binary_path);
        }
        std::fs::rename(&temp_path, &binary_path).context("Failed to rename downloaded binary")?;

        // Save version info
        let version_path = install_dir.join("copilot-language-server.version");
        let _ = std::fs::write(&version_path, tag);

        log::info!(
            "Copilot language server v{} installed to: {:?} ({} bytes)",
            tag,
            binary_path,
            binary_data.len()
        );

        Ok(CopilotBinary::Executable(binary_path))
    }

    // ══════════════════════════════════════════════════════════════════════
    // Lifecycle
    // ══════════════════════════════════════════════════════════════════════

    pub async fn new(oauth_token: String) -> Result<Self> {
        let binary = match Self::find_binary() {
            Ok(b) => b,
            Err(_) => {
                log::info!(
                    "Copilot binary not found — auto-downloading from \
                     copilot-language-server-release..."
                );
                match Self::auto_download_binary().await {
                    Ok(b) => {
                        log::info!("Auto-download succeeded.");
                        b
                    }
                    Err(e) => {
                        bail!(
                            "Copilot agent not found and auto-download failed:\n  {}\n\
                             \n\
                             To fix manually:\n\
                             1. Download from:\n\
                                https://github.com/github/copilot-language-server-release/releases\n\
                             2. Place it in your PATH, or\n\
                             3. Set COPILOT_AGENT_PATH to its location.",
                            e
                        );
                    }
                }
            }
        };

        Self::new_with_binary(oauth_token, binary).await
    }

    async fn new_with_binary(oauth_token: String, binary: CopilotBinary) -> Result<Self> {
        let mut child = match binary {
            CopilotBinary::Executable(ref path) => {
                log::debug!("Spawning standalone Copilot agent: {:?}", path);
                Command::new(path)
                    .arg("--stdio")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .context("Failed to spawn standalone Copilot agent")?
            }
            CopilotBinary::NodeAgent(ref path) => {
                let node_path = Self::find_node()?;
                log::debug!("Spawning Node.js agent: {:?} {:?}", node_path, path);
                Command::new(&node_path)
                    .arg(path)
                    .arg("--stdio")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .context("Failed to spawn Node.js Copilot agent")?
            }
        };

        let pid = child.id().unwrap_or(0);
        log::debug!("Agent process spawned with PID: {}", pid);

        let stdin = child.stdin.take().context("No stdin")?;
        let stdout = child.stdout.take().context("No stdout")?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (write_tx, write_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        tokio::spawn(stdin_writer(stdin, write_rx));
        tokio::spawn(stdout_reader(BufReader::new(stdout), pending.clone()));

        // ── Capture stderr for diagnostics ────────────────────────
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    log::debug!("[copilot-agent stderr] {}", line);
                }
            });
        }

        let session_id = uuid::Uuid::new_v4().to_string();

        let server = Self {
            _process: child,
            write_tx,
            pending,
            request_id: AtomicU64::new(1),
            oauth_token,
            session_id,
        };

        // ── LSP Handshake ──────────────────────────────────────────
        let init_result =
            tokio::time::timeout(std::time::Duration::from_secs(15), server.initialize())
                .await
                .context(
                    "Timeout waiting for Copilot agent to initialize (15s). \
                     The binary may be corrupted or incompatible with this platform.",
                )??;

        log::debug!("Initialize result: {:?}", init_result);

        server.initialized_notification().await?;
        server.set_editor_info().await?;

        log::debug!("Copilot agent spawned. Auth checked by background task.");
        Ok(server)
    }

    // ══════════════════════════════════════════════════════════════════════
    // LSP handshake methods
    // ══════════════════════════════════════════════════════════════════════

    async fn initialize(&self) -> Result<Value> {
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": null,
            "capabilities": {
                "textDocument": {
                    "inlineCompletion": {
                        "dynamicRegistration": false
                    }
                }
            },
            "initializationOptions": {
                "editorInfo": { "name": "ce", "version": "0.1.0" },
                "editorPluginInfo": { "name": "copilot-cli", "version": "0.1.0" }
            },
        });
        self.send_request("initialize", Some(params)).await
    }

    async fn initialized_notification(&self) -> Result<()> {
        self.send_notification("initialized", Some(serde_json::json!({})))
            .await
    }

    async fn set_editor_info(&self) -> Result<()> {
        let params = serde_json::json!({
            "editorInfo": { "name": "ce", "version": "0.1.0" },
            "editorPluginInfo": { "name": "copilot-cli", "version": "0.1.0" }
        });
        self.send_notification("setEditorInfo", Some(params)).await
    }

    /// Check agent authentication status.  Also triggers "priming token"
    /// from `~/.config/github-copilot/hosts.json`.
    async fn check_status(&self) -> Result<Value> {
        self.send_request("checkStatus", Some(serde_json::json!({})))
            .await
    }

    /// Trigger re-reading of `hosts.json` and token validation.
    /// Called after `:copilot-auth` saves a fresh token.
    async fn sign_in_initiate(&self) -> Result<Value> {
        self.send_request("signInInitiate", Some(serde_json::json!({})))
            .await
    }

    async fn did_open(&self, uri: &str, language_id: &str, text: &str) -> Result<()> {
        let params = serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 0,
                "text": text
            }
        });
        self.send_notification("textDocument/didOpen", Some(params))
            .await
    }

    // ══════════════════════════════════════════════════════════════════════
    // Completions
    // ══════════════════════════════════════════════════════════════════════

    pub async fn fetch_completion_items(
        &self,
        full_text: &str,
        cursor_offset: usize,
        language: &str,
    ) -> Result<Vec<String>> {
        let (line, character) = byte_offset_to_position(full_text, cursor_offset);
        let uri = format!("file:///copilot-cli/{}.{}", uuid::Uuid::new_v4(), language);
        let relative_path = format!("snippet.{}", language);

        // The Copilot agent requires textDocument/didOpen before it will
        // accept getCompletions for a URI.  Register the document first.
        self.did_open(&uri, language, full_text).await?;

        let doc_position = serde_json::json!({ "line": line, "character": character });

        let params = serde_json::json!({
            "doc": {
                "source": full_text,
                "tabSize": 4,
                "indentSize": 4,
                "insertSpaces": true,
                "path": uri,
                "uri": uri,
                "relativePath": relative_path,
                "languageId": language,
                "position": doc_position,
                "version": 0,
            },
            "position": doc_position
        });

        log::debug!(
            "Requesting completions. Language: {}, Position: {}:{}",
            language,
            line,
            character
        );

        let result = self.send_request("getCompletions", Some(params)).await?;

        let mut completions = Vec::new();
        if let Some(items) = result.get("completions").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        completions.push(text.to_string());
                    }
                }
            }
        }

        log::debug!(
            "Received {} completion(s) from Copilot agent.",
            completions.len()
        );
        Ok(completions)
    }

    // ══════════════════════════════════════════════════════════════════════
    // Low-level LSP I/O
    // ══════════════════════════════════════════════════════════════════════

    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let mut msg = serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        self.write_lsp_message(&msg).await?;

        tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .context("Timeout waiting for LSP response")?
            .context("LSP response channel closed")?
    }

    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        let mut msg = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.write_lsp_message(&msg).await
    }

    async fn write_lsp_message(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut frame = header.into_bytes();
        frame.extend_from_slice(body.as_bytes());
        self.write_tx
            .send(frame)
            .context("LSP stdin writer closed")?;
        Ok(())
    }
}

impl Drop for CopilotServer {
    fn drop(&mut self) {
        log::debug!("Dropping CopilotServer; killing agent process.");
        let _ = self._process.start_kill();
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Background I/O tasks
// ──────────────────────────────────────────────────────────────────────────

async fn stdin_writer(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(frame) = rx.recv().await {
        if stdin.write_all(&frame).await.is_err() {
            break;
        }
        if stdin.flush().await.is_err() {
            break;
        }
    }
}

async fn stdout_reader(
    mut reader: BufReader<ChildStdout>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
) {
    loop {
        match read_lsp_message(&mut reader).await {
            Ok(msg) => {
                if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                    let mut map = pending.lock().await;
                    if let Some(sender) = map.remove(&id) {
                        let result = if let Some(error) = msg.get("error") {
                            let code = error.get("code").and_then(|c| c.as_i64());
                            let message = error
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("Unknown LSP error");
                            Err(anyhow::anyhow!("LSP error {:?}: {}", code, message))
                        } else {
                            Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = sender.send(result);
                    }
                } else if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                    if method == "window/logMessage" {
                        if let Some(t) = msg
                            .get("params")
                            .and_then(|p| p.get("message"))
                            .and_then(|m| m.as_str())
                        {
                            log::debug!("Agent log: {}", t);
                        }
                    }
                }
            }
            Err(e) => {
                log::debug!("LSP reader error: {}", e);
                let mut map = pending.lock().await;
                for (_, sender) in map.drain() {
                    let _ = sender.send(Err(anyhow::anyhow!("Agent process exited")));
                }
                break;
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// LSP framing
// ──────────────────────────────────────────────────────────────────────────

async fn read_lsp_message(reader: &mut BufReader<ChildStdout>) -> Result<Value> {
    let mut content_length: Option<usize> = None;
    let mut header_line = String::new();

    loop {
        header_line.clear();
        let n = reader
            .read_line(&mut header_line)
            .await
            .context("LSP header read")?;
        if n == 0 {
            bail!("LSP stream closed (EOF)");
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(
                rest.trim()
                    .parse::<usize>()
                    .context("Invalid Content-Length")?,
            );
        }
    }

    let length = content_length.context("Missing Content-Length")?;
    let mut buffer = vec![0u8; length];
    reader
        .read_exact(&mut buffer)
        .await
        .context("LSP body read")?;
    let body = String::from_utf8(buffer).context("LSP body not UTF-8")?;
    serde_json::from_str(&body).context("Invalid LSP JSON")
}

// ──────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────

fn byte_offset_to_position(text: &str, offset: usize) -> (u32, u32) {
    let mut line = 0u32;
    let mut char_offset = 0u32;
    let mut bytes_seen = 0usize;
    for ch in text.chars() {
        if bytes_seen >= offset {
            break;
        }
        bytes_seen += ch.len_utf8();
        if ch == '\n' {
            line += 1;
            char_offset = 0;
        } else {
            char_offset += 1;
        }
    }
    (line, char_offset)
}

// ──────────────────────────────────────────────────────────────────────────
// Channel-based interface
// ──────────────────────────────────────────────────────────────────────────

/// Requests sent from the editor to the Copilot background task.
pub enum CopilotRequest {
    /// Request inline completions for the given document state.
    Completion {
        text: String,
        offset: usize,
        language: String,
        version: u64,
    },
    /// Re-prime the agent after `:copilot-auth` saves a new token
    /// to `hosts.json`.  Calls `signInInitiate` which triggers
    /// re-reading the token file.
    Reauth,
}

/// Responses from the Copilot background task to the editor.
pub enum CopilotResponse {
    /// Successful completion results.
    Items { version: u64, items: Vec<String> },
    /// Completion request failed.
    Error { version: u64, error: String },
    /// Result of a `Reauth` request.
    ReauthResult { ok: bool },
}

pub type CopilotServerCell = std::sync::Arc<std::sync::RwLock<Option<CopilotHandle>>>;

#[derive(Clone)]
pub struct CopilotHandle {
    pub request_tx: tokio::sync::mpsc::UnboundedSender<CopilotRequest>,
    pub ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Spawn the Copilot agent in a background tokio task.
///
/// On startup, calls `checkStatus` which triggers the agent's
/// "priming token" from `~/.config/github-copilot/hosts.json`.
/// If the token is valid, `ready` is set to `true` and completions
/// flow.  If not, completions are rejected fast and the user must
/// run `:copilot-auth` to save a fresh token, then send a `Reauth`
/// request to re-prime the agent.
pub fn start_copilot_server_task(
    oauth_token: String,
) -> (CopilotHandle, std::sync::mpsc::Receiver<CopilotResponse>) {
    let (req_tx, mut req_rx) = tokio::sync::mpsc::unbounded_channel::<CopilotRequest>();
    let (resp_tx, resp_rx) = std::sync::mpsc::channel::<CopilotResponse>();
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let handle = CopilotHandle {
        request_tx: req_tx,
        ready: ready.clone(),
    };

    let ready_flag = ready.clone();

    tokio::spawn(async move {
        log::debug!("Copilot background task: starting agent…");

        let server = match CopilotServer::new(oauth_token).await {
            Ok(s) => s,
            Err(e) => {
                log::error!("Copilot agent failed to start: {}", e);
                return;
            }
        };

        // ── Single auth check on startup ──────────────────────────
        // checkStatus triggers "priming token" from hosts.json.
        // If it fails, the token is invalid — user must run :copilot-auth.
        let auth_ok = match server.check_status().await {
            Ok(status) => {
                let s = status
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                if s == "OK" || s == "AlreadySignedIn" {
                    log::info!("Copilot authenticated ({})", s);
                    true
                } else {
                    log::warn!("Copilot not authenticated ({}). Run :copilot-auth.", s);
                    false
                }
            }
            Err(e) => {
                log::warn!("Copilot checkStatus failed: {}. Run :copilot-auth.", e);
                false
            }
        };
        ready_flag.store(auth_ok, Ordering::Relaxed);

        // ── Main request loop ─────────────────────────────────────
        while let Some(req) = req_rx.recv().await {
            match req {
                CopilotRequest::Completion {
                    text,
                    offset,
                    language,
                    version,
                } => {
                    if !ready_flag.load(Ordering::Relaxed) {
                        let _ = resp_tx.send(CopilotResponse::Error {
                            version,
                            error: "Copilot not authenticated".into(),
                        });
                        continue;
                    }

                    let result = server
                        .fetch_completion_items(&text, offset, &language)
                        .await;

                    let msg = match result {
                        Ok(items) => CopilotResponse::Items { version, items },
                        Err(e) => {
                            log::debug!("Copilot completion error: {}", e);
                            CopilotResponse::Error {
                                version,
                                error: e.to_string(),
                            }
                        }
                    };

                    if resp_tx.send(msg).is_err() {
                        break;
                    }
                }

                CopilotRequest::Reauth => {
                    // signInInitiate triggers re-reading hosts.json
                    let ok = match server.sign_in_initiate().await {
                        Ok(result) => {
                            let s = result
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown");
                            s == "OK" || s == "AlreadySignedIn"
                        }
                        Err(e) => {
                            log::debug!("Reauth signInInitiate failed: {}", e);
                            false
                        }
                    };

                    if ok {
                        log::info!("Copilot re-authenticated after :copilot-auth.");
                    } else {
                        log::warn!("Copilot re-auth failed — token may still be invalid.");
                    }

                    ready_flag.store(ok, Ordering::Relaxed);

                    let _ = resp_tx.send(CopilotResponse::ReauthResult { ok });
                }
            }
        }

        ready_flag.store(false, Ordering::Relaxed);
        log::debug!("Copilot background task finished.");
    });

    (handle, resp_rx)
}
