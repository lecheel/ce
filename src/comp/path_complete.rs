use std::path::Path;

/// Completes a filesystem path given a prefix strictly starting with "./".
pub fn complete_path(prefix: &str) -> Vec<String> {
    if !prefix.starts_with("./") {
        return Vec::new();
    }

    let relative = &prefix[2..];

    let (dir, partial_file) = if relative.is_empty() {
        (Path::new(".").to_path_buf(), String::new())
    } else if relative.ends_with('/') {
        (Path::new(relative).to_path_buf(), String::new())
    } else {
        let path = Path::new(relative);
        (
            path.parent()
                .map(|p| {
                    if p.as_os_str().is_empty() {
                        Path::new(".").to_path_buf()
                    } else {
                        p.to_path_buf()
                    }
                })
                .unwrap_or_else(|| Path::new(".").to_path_buf()),
            path.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
    };

    let mut matches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();

            if file_name.starts_with(&partial_file) {
                if file_name.starts_with('.') && !partial_file.starts_with('.') {
                    continue;
                }

                let is_dir = entry.path().is_dir();
                let mut completion = prefix.to_string();

                if relative.ends_with('/') || partial_file.is_empty() {
                    completion.push_str(&file_name);
                } else {
                    let base_len = prefix.len() - partial_file.len();
                    completion.truncate(base_len);
                    completion.push_str(&file_name);
                }

                if is_dir {
                    completion.push('/');
                }
                matches.push(completion);
            }
        }
    }

    matches.sort();
    matches
}
