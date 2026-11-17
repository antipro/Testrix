use std::path::{Path, PathBuf};

/// Converts an arbitrary display name into a clean lowercase filesystem slug.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = false;

    for ch in input.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// Chooses a unique file path inside `dir` with the given `suffix` (e.g. `.request.yaml`).
pub(crate) fn unique_file_path(dir: &Path, name: &str, suffix: &str) -> PathBuf {
    let base = slugify(name);
    let candidate = dir.join(format!("{base}{suffix}"));
    if !candidate.exists() {
        return candidate;
    }

    for counter in 2..10_000 {
        let numbered = dir.join(format!("{base}-{counter}{suffix}"));
        if !numbered.exists() {
            return numbered;
        }
    }

    dir.join(format!("{base}-{}{suffix}", std::process::id()))
}

/// Chooses a unique directory path inside `parent`.
pub(crate) fn unique_dir_path(parent: &Path, name: &str) -> PathBuf {
    let base = slugify(name);
    let candidate = parent.join(&base);
    if !candidate.exists() {
        return candidate;
    }

    for counter in 2..10_000 {
        let numbered = parent.join(format!("{base}-{counter}"));
        if !numbered.exists() {
            return numbered;
        }
    }

    parent.join(format!("{base}-{}", std::process::id()))
}
