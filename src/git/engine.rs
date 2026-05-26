// src/git/engine.rs
use crate::ed::buffer::GitSign;
use git2::{DiffOptions, Patch, Repository};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct GitEngine {
    repo_root: PathBuf,
}

impl GitEngine {
    pub fn new(repo_root: PathBuf) -> Self {
        log::debug!("GitEngine: initialized with repo_root={:?}", repo_root);
        Self { repo_root }
    }

    /// Retrieves the raw file content from HEAD for the specified relative path.
    pub fn get_head_blob_content(&self, relative_path: &Path) -> Option<Vec<u8>> {
        log::debug!("GitEngine: fetching HEAD blob for {:?}", relative_path);
        let repo = Repository::open(&self.repo_root).ok()?;
        let head = repo.head().ok()?;
        let commit = head.peel_to_commit().ok()?;
        let tree = commit.tree().ok()?;
        let entry = tree.get_path(relative_path).ok()?;
        let obj = entry.to_object(&repo).ok()?;
        let blob = obj.into_blob().ok()?;
        let content = blob.content().to_vec();
        log::debug!(
            "GitEngine: HEAD blob for {:?} is {} bytes",
            relative_path,
            content.len()
        );
        Some(content)
    }

    /// Computes line updates by comparing the HEAD reference to the in-memory buffer.
    pub fn compute_gutter_diff(
        &self,
        relative_path: &Path,
        current_buffer: &[u8],
    ) -> Option<HashMap<usize, GitSign>> {
        log::debug!(
            "GitEngine: computing gutter diff for {:?} (buffer={} bytes)",
            relative_path,
            current_buffer.len()
        );

        let _repo = Repository::open(&self.repo_root).ok()?;
        let head_content = self
            .get_head_blob_content(relative_path)
            .unwrap_or_default();

        let mut diffs = HashMap::new();
        let mut opts = DiffOptions::new();
        opts.context_lines(0); // Exclude matching surrounding context

        // Compute comparison directly between reference buffer and active buffer
        let patch = Patch::from_buffers(
            &head_content,
            Some(relative_path),
            current_buffer,
            Some(relative_path),
            Some(&mut opts),
        )
        .ok()?;

        let num_hunks = patch.num_hunks();
        log::debug!("GitEngine: computed patch with {} hunks", num_hunks);

        for h in 0..num_hunks {
            if let Ok(num_lines) = patch.num_lines_in_hunk(h) {
                log::debug!("GitEngine: hunk {} has {} lines", h, num_lines);
                for l in 0..num_lines {
                    if let Ok(line) = patch.line_in_hunk(h, l) {
                        match line.origin() {
                            '+' => {
                                if let Some(new_ln) = line.new_lineno() {
                                    diffs.insert(new_ln as usize - 1, GitSign::Added);
                                }
                            }
                            '-' => {
                                if let Some(new_ln) = line.new_lineno() {
                                    diffs.insert(new_ln as usize - 1, GitSign::Removed);
                                } else if let Some(old_ln) = line.old_lineno() {
                                    diffs.insert(old_ln as usize - 1, GitSign::Removed);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        log::debug!(
            "GitEngine: gutter diff complete, {} signs mapped",
            diffs.len()
        );
        Some(diffs)
    }
}
