//! Client for the `ctagd` Language Server daemon.
//!
//! Communicates via NDJSON over a Unix domain socket at
//! `/tmp/.ctagd.sock`.  See `client.md` for the full protocol spec.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// ── Constants ────────────────────────────────────────────────────────────

const DEFAULT_SOCKET_PATH: &str = "/tmp/.ctagd.sock";
const CONNECT_TIMEOUT_MS: u64 = 300;
const READ_TIMEOUT_MS: u64 = 1500;
const WRITE_TIMEOUT_MS: u64 = 200;

// ── ID counter ───────────────────────────────────────────────────────────

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> String {
    format!("ce-{}", REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Status information returned by the `info` method.
#[derive(Debug, Clone)]
pub struct DaemonInfo {
    pub repo_root: String,
    pub backend: String,
    pub index_status: String,
    pub indexed_files: usize,
    pub indexed_symbols: usize,
}

/// Information about a single repo session in the daemon.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub repo_root: String,
    pub backend: String,
    pub index_status: String,
    pub indexed_files: usize,
    pub indexed_symbols: usize,
}

// ── Result types ─────────────────────────────────────────────────────────

/// A single definition location returned by `definition` or `goto`.
#[derive(Debug, Clone)]
pub struct DefinitionResult {
    /// File path *relative* to `repo_root`.
    pub file: String,
    /// 0-based line number.
    pub line: usize,
    /// 0-based column offset.
    pub column: usize,
    /// Human-readable display string (e.g. `"fn my_func()"`).
    pub display: Option<String>,
}

/// A symbol entry returned by `workspace_symbols`.
#[derive(Debug, Clone)]
pub struct SymbolResult {
    pub name: String,
    pub kind: String,
    /// File path *relative* to `repo_root`.
    pub relative_path: String,
    /// 0-based line number.
    pub line: usize,
    /// 0-based column offset.
    pub column: usize,
    pub detail: Option<String>,
}

// ── Client ───────────────────────────────────────────────────────────────

/// Client for the `ctagd` daemon.
pub struct RustLspClient {
    socket_path: PathBuf,
    available: bool,
}

impl RustLspClient {
    pub fn new() -> Self {
        let socket_path = PathBuf::from(DEFAULT_SOCKET_PATH);
        let available = socket_path.exists();
        Self {
            socket_path,
            available,
        }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    pub fn refresh_availability(&mut self) {
        self.available = self.socket_path.exists();
    }

    // ── Fire-and-forget: `saved` ──────────────────────────────────────

    pub fn notify_saved(&self, repo_root: &Path, file: &str, content: &str) {
        if !self.available {
            return;
        }

        let payload = serde_json::json!({
            "id": next_request_id(),
            "method": "saved",
            "repo_root": repo_root.to_string_lossy().as_ref(),
            "file": file,
            "content": content,
        });

        let socket_path = self.socket_path.clone();
        std::thread::spawn(move || {
            if let Ok(mut stream) = UnixStream::connect(&socket_path) {
                let _ = stream.set_write_timeout(Some(Duration::from_millis(WRITE_TIMEOUT_MS)));
                let _ = write!(stream, "{}\n", payload);
            }
        });
    }

    // ── Blocking queries ──────────────────────────────────────────────

    pub fn definition(
        &mut self,
        repo_root: &Path,
        file: &str,
        line: usize,
        column: usize,
        symbol: &str,
    ) -> Option<DefinitionResult> {
        if !self.available {
            return None;
        }

        let payload = serde_json::json!({
            "id": next_request_id(),
            "method": "definition",
            "repo_root": repo_root.to_string_lossy().as_ref(),
            "file": file,
            "line": line,
            "column": column,
            "symbol": symbol,
        });

        match self.send_and_receive(&payload) {
            Some(result) => self.parse_definition_result(result),
            None => {
                self.available = false;
                None
            }
        }
    }

    pub fn goto(&mut self, repo_root: &Path, query: &str) -> Option<DefinitionResult> {
        if !self.available {
            return None;
        }

        let payload = serde_json::json!({
            "id": next_request_id(),
            "method": "goto",
            "repo_root": repo_root.to_string_lossy().as_ref(),
            "query": query,
        });

        match self.send_and_receive(&payload) {
            Some(result) => self.parse_definition_result(result),
            None => {
                self.available = false;
                None
            }
        }
    }

    pub fn workspace_symbols(
        &mut self,
        repo_root: &Path,
        query: &str,
    ) -> Option<Vec<SymbolResult>> {
        if !self.available {
            return None;
        }

        let payload = serde_json::json!({
            "id": next_request_id(),
            "method": "workspace_symbols",
            "repo_root": repo_root.to_string_lossy().as_ref(),
            "query": query,
        });

        match self.send_and_receive(&payload) {
            Some(result) => self.parse_workspace_symbols(result),
            None => {
                self.available = false;
                None
            }
        }
    }

    // ── Private ───────────────────────────────────────────────────────

    fn send_and_receive(&self, payload: &serde_json::Value) -> Option<serde_json::Value> {
        let stream = UnixStream::connect(&self.socket_path).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(READ_TIMEOUT_MS)))
            .ok()?;
        stream
            .set_write_timeout(Some(Duration::from_millis(CONNECT_TIMEOUT_MS)))
            .ok()?;

        let mut stream = stream;
        write!(stream, "{}\n", payload).ok()?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;

        if line.trim().is_empty() {
            return None;
        }

        let resp: serde_json::Value = serde_json::from_str(&line).ok()?;
        resp.get("result").cloned()
    }

    fn parse_definition_result(&self, result: serde_json::Value) -> Option<DefinitionResult> {
        let obj = result.as_object()?;
        Some(DefinitionResult {
            file: obj.get("file")?.as_str()?.to_string(),
            line: obj.get("line")?.as_u64()? as usize,
            column: obj.get("column")?.as_u64()? as usize,
            display: obj
                .get("display")
                .and_then(|v| v.as_str())
                .map(String::from),
        })
    }

    fn parse_workspace_symbols(&self, result: serde_json::Value) -> Option<Vec<SymbolResult>> {
        let arr = result.as_array()?;
        let mut symbols = Vec::with_capacity(arr.len());
        for item in arr {
            let obj = item.as_object()?;
            symbols.push(SymbolResult {
                name: obj.get("name")?.as_str()?.to_string(),
                kind: obj
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                relative_path: obj.get("relative_path")?.as_str()?.to_string(),
                line: obj.get("line")?.as_u64()? as usize,
                column: obj.get("column")?.as_u64()? as usize,
                detail: obj.get("detail").and_then(|v| v.as_str()).map(String::from),
            });
        }
        Some(symbols)
    }

    /// `info` — query daemon status for a repo.
    pub fn info(&mut self, repo_root: &Path) -> Option<DaemonInfo> {
        if !self.available {
            return None;
        }

        let payload = serde_json::json!({
            "id": next_request_id(),
            "method": "info",
            "repo_root": repo_root.to_string_lossy().as_ref(),
        });

        match self.send_and_receive(&payload) {
            Some(result) => self.parse_info_result(result),
            None => {
                self.available = false;
                None
            }
        }
    }

    fn parse_info_result(&self, result: serde_json::Value) -> Option<DaemonInfo> {
        let obj = result.as_object()?;
        Some(DaemonInfo {
            repo_root: obj.get("repo_root")?.as_str()?.to_string(),
            backend: obj
                .get("backend")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            index_status: obj
                .get("index_status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            indexed_files: obj
                .get("indexed_files")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            indexed_symbols: obj
                .get("indexed_symbols")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
        })
    }

    /// `sessions` — list all active repo sessions in the daemon.
    /// Does not require a specific repo_root; sends an empty one.
    pub fn sessions(&mut self) -> Option<Vec<SessionInfo>> {
        if !self.available {
            return None;
        }

        let payload = serde_json::json!({
            "id": next_request_id(),
            "method": "sessions",
            "repo_root": "",
        });

        match self.send_and_receive(&payload) {
            Some(result) => self.parse_sessions_result(result),
            None => {
                self.available = false;
                None
            }
        }
    }

    fn parse_sessions_result(&self, result: serde_json::Value) -> Option<Vec<SessionInfo>> {
        let arr = result.as_array()?;
        let mut sessions = Vec::with_capacity(arr.len());
        for item in arr {
            let obj = item.as_object()?;
            sessions.push(SessionInfo {
                repo_root: obj.get("repo_root")?.as_str()?.to_string(),
                backend: obj
                    .get("backend")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                index_status: obj
                    .get("index_status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                indexed_files: obj
                    .get("indexed_files")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
                indexed_symbols: obj
                    .get("indexed_symbols")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
            });
        }
        Some(sessions)
    }

    /// `scan` — trigger a full repo re-index.
    pub fn scan(&mut self, repo_root: &Path) -> bool {
        if !self.available {
            return false;
        }

        let payload = serde_json::json!({
            "id": next_request_id(),
            "method": "scan",
            "repo_root": repo_root.to_string_lossy().as_ref(),
        });

        match self.send_and_receive(&payload) {
            Some(_) => true,
            None => {
                self.available = false;
                false
            }
        }
    }
}
