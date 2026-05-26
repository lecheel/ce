//! Async gutter worker — background git diff computation using git2-rs.

use crate::ed::buffer::GitSign;
use git2::DiffOptions;
use ropey::Rope;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

/// Result of a background diff computation for a single buffer.
pub struct GutterResponse {
    pub buffer_id: usize,
    pub diffs: HashMap<usize, GitSign>,
}

struct GutterRequest {
    buffer_id: usize,
    rope: Rope,
    filename: String,
}

/// Async gutter worker managing the background diffing thread.
pub struct AsyncGutterWorker {
    sender: std::sync::mpsc::Sender<GutterRequest>,
    receiver: std::sync::mpsc::Receiver<GutterResponse>,
}

impl AsyncGutterWorker {
    pub fn new() -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel();
        let (res_tx, res_rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            for req in req_rx {
                let res = compute_diff(req);
                if res_tx.send(res).is_err() {
                    break;
                }
            }
        });

        Self {
            sender: req_tx,
            receiver: res_rx,
        }
    }

    /// Enqueue a buffer for background diffing.
    pub fn request_diff(&self, buffer_id: usize, rope: &Rope, filename: Option<&str>) {
        if let Some(fname) = filename {
            log::debug!(
                "GutterWorker: requesting diff for buffer_id={}, file={}",
                buffer_id,
                fname
            );
            let req = GutterRequest {
                buffer_id,
                rope: rope.clone(),
                filename: fname.to_string(),
            };
            if let Err(e) = self.sender.send(req) {
                log::warn!("GutterWorker: failed to send diff request: {}", e);
            }
        } else {
            log::debug!(
                "GutterWorker: skipping diff request for buffer_id={} (no filename)",
                buffer_id
            );
        }
    }

    /// Poll for completed background diff results.
    pub fn poll_results(&self) -> Vec<GutterResponse> {
        let mut results = Vec::new();
        while let Ok(res) = self.receiver.try_recv() {
            log::debug!(
                "GutterWorker: received diff result for buffer_id={}, signs={}",
                res.buffer_id,
                res.diffs.len()
            );
            results.push(res);
        }
        results
    }
}

/// Core diffing logic running on the background thread.
fn compute_diff(req: GutterRequest) -> GutterResponse {
    let mut diffs = HashMap::new();

    let path = Path::new(&req.filename);
    let abs_filename = if path.is_absolute() {
        path.to_path_buf()
    } else {
        path.canonicalize().unwrap_or_else(|e| {
            log::warn!(
                "GutterWorker: canonicalize failed for '{}': {}",
                req.filename,
                e
            );
            std::env::current_dir().unwrap_or_default().join(path)
        })
    };

    // 1. Open repository
    let repo = match git2::Repository::discover(&abs_filename) {
        Ok(r) => {
            log::debug!(
                "GutterWorker: discovered git repo at {:?} for file {:?}",
                r.path(),
                abs_filename
            );
            r
        }
        Err(e) => {
            log::debug!(
                "GutterWorker: no git repo found for file {:?}: {}",
                abs_filename,
                e
            );
            return GutterResponse {
                buffer_id: req.buffer_id,
                diffs,
            };
        }
    };

    // 2. Get HEAD commit and tree
    let head_obj = repo.revparse_single("HEAD").ok();
    let head_commit = head_obj.as_ref().and_then(|obj| obj.as_commit());
    let head_tree = head_commit.and_then(|c| c.tree().ok());

    if head_tree.is_none() {
        log::debug!(
            "GutterWorker: could not resolve HEAD tree for {:?}",
            abs_filename
        );
    }

    // 3. Get relative path
    let workdir = repo.workdir().unwrap_or(Path::new("."));
    let rel_path = abs_filename
        .strip_prefix(workdir)
        .unwrap_or(&abs_filename)
        .to_path_buf();

    log::debug!(
        "GutterWorker: workdir={:?}, rel_path={:?}",
        workdir,
        rel_path
    );

    // 4. Get old blob content (empty string if file is untracked or repo is fresh)
    let old_content = head_tree
        .and_then(|tree| {
            tree.get_path(&rel_path).ok().and_then(|entry| {
                entry.to_object(&repo).ok().and_then(|obj| {
                    obj.as_blob()
                        .map(|blob| String::from_utf8_lossy(blob.content()).to_string())
                })
            })
        })
        .unwrap_or_default();

    if old_content.is_empty() {
        log::debug!(
            "GutterWorker: no HEAD content for {:?} (new/untracked file)",
            rel_path
        );
    } else {
        log::debug!(
            "GutterWorker: loaded HEAD content for {:?} ({} bytes)",
            rel_path,
            old_content.len()
        );
    }

    let new_content = req.rope.to_string();
    log::debug!(
        "GutterWorker: current buffer size={} bytes",
        new_content.len()
    );

    // 5. Compute Patch in-memory using git2::Patch::from_buffers
    let mut opts = DiffOptions::new();
    opts.context_lines(0); // Equivalent to `git diff -U0`

    match git2::Patch::from_buffers(
        old_content.as_bytes(),
        Some(rel_path.as_path()),
        new_content.as_bytes(),
        Some(rel_path.as_path()),
        Some(&mut opts),
    ) {
        Ok(patch) => {
            let num_hunks = patch.num_hunks();
            log::debug!("GutterWorker: computed patch with {} hunks", num_hunks);

            for i in 0..num_hunks {
                if let Ok((hunk, _)) = patch.hunk(i) {
                    let old_lines = hunk.old_lines() as usize;
                    let new_lines = hunk.new_lines() as usize;
                    let new_start = hunk.new_start() as usize; // 1-based

                    log::debug!(
                        "GutterWorker: hunk {} — old_lines={}, new_lines={}, new_start={}",
                        i,
                        old_lines,
                        new_lines,
                        new_start
                    );

                    if new_lines > 0 && old_lines > 0 {
                        let mods = new_lines.min(old_lines);
                        for l in 0..new_lines {
                            let line_idx = new_start + l - 1; // 0-based
                            if l < mods {
                                diffs.insert(line_idx, GitSign::Modified);
                            } else {
                                diffs.insert(line_idx, GitSign::Added);
                            }
                        }
                        if old_lines > new_lines {
                            let del_line_idx = new_start + new_lines - 2;
                            diffs.entry(del_line_idx).or_insert(GitSign::Removed);
                        }
                    } else if new_lines > 0 && old_lines == 0 {
                        for l in 0..new_lines {
                            let line_idx = new_start + l - 1;
                            diffs.insert(line_idx, GitSign::Added);
                        }
                    } else if new_lines == 0 && old_lines > 0 {
                        let del_line_idx = if new_start > 0 { new_start - 1 } else { 0 };
                        diffs.entry(del_line_idx).or_insert(GitSign::Removed);
                    }
                } else {
                    log::warn!("GutterWorker: failed to parse hunk {}", i);
                }
            }

            log::debug!(
                "GutterWorker: mapped {} sign entries for buffer_id={}",
                diffs.len(),
                req.buffer_id
            );
        }
        Err(e) => {
            log::warn!(
                "GutterWorker: Patch::from_buffers failed for {:?}: {}",
                rel_path,
                e
            );
        }
    }

    GutterResponse {
        buffer_id: req.buffer_id,
        diffs,
    }
}

pub fn find_git_root(start_dir: &Path) -> Option<PathBuf> {
    let effective_dir = if start_dir.is_file() {
        start_dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| Path::new(".").to_path_buf())
    } else if start_dir.exists() {
        start_dir.to_path_buf()
    } else {
        start_dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| Path::new(".").to_path_buf())
    };

    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&effective_dir)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if path_str.is_empty() {
                None
            } else {
                let p = PathBuf::from(path_str);
                Some(std::fs::canonicalize(&p).unwrap_or(p))
            }
        })
}
