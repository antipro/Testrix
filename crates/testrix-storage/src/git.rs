use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::StorageError;

const LOCAL_IGNORE_PATTERNS: &[&str] = &[
    ".testrix-state.yaml",
    ".testrix-history.yaml",
    ".testrix-responses/",
    ".DS_Store",
    "*.tmp.*",
];

/// Metadata for a single git commit touching a workspace or file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub timestamp: i64,
    pub message: String,
    pub file_path_at_commit: Option<String>,
}

fn git_cmd(workspace_root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(workspace_root);
    cmd
}

fn run_git_output(workspace_root: &Path, args: &[&str]) -> Result<std::process::Output, StorageError> {
    git_cmd(workspace_root)
        .args(args)
        .output()
        .map_err(|e| StorageError::Git(format!("failed to invoke git {:?}: {e}", args)))
}

fn run_git_checked(workspace_root: &Path, args: &[&str]) -> Result<String, StorageError> {
    let output = run_git_output(workspace_root, args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(StorageError::Git(format!(
            "git {} failed: {detail}",
            args.join(" ")
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_config_exists(workspace_root: &Path, key: &str) -> bool {
    run_git_output(workspace_root, &["config", "--get", key])
        .map(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

fn run_git_commit(workspace_root: &Path, message: &str) -> Result<String, StorageError> {
    let mut cmd = git_cmd(workspace_root);
    if !git_config_exists(workspace_root, "user.name") {
        cmd.arg("-c").arg("user.name=Testrix");
    }
    if !git_config_exists(workspace_root, "user.email") {
        cmd.arg("-c").arg("user.email=testrix@local");
    }
    let output = cmd
        .args(["commit", "-m", message])
        .output()
        .map_err(|e| StorageError::Git(format!("failed to invoke git commit: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(StorageError::Git(format!("git commit failed: {detail}")));
    }

    head_commit_hash(workspace_root)?
        .ok_or_else(|| StorageError::Git("HEAD missing after git commit".to_string()))
}

fn ensure_lines_in_file(path: &Path, patterns: &[&str]) -> Result<(), StorageError> {
    let existing = if path.is_file() {
        fs::read_to_string(path).map_err(|source| StorageError::Io {
            path: path.to_path_buf(),
            source,
        })?
    } else {
        String::new()
    };

    let existing_lines: Vec<&str> = existing.lines().map(str::trim).collect();
    let mut missing = Vec::new();
    for pat in patterns {
        if !existing_lines.contains(pat) {
            missing.push(*pat);
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| StorageError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    for pat in missing {
        next.push_str(pat);
        next.push('\n');
    }

    fs::write(path, next.as_bytes()).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn resolve_git_dir(workspace_root: &Path) -> Result<PathBuf, StorageError> {
    let raw = run_git_checked(workspace_root, &["rev-parse", "--git-dir"])?;
    let trimmed = raw.trim();
    let p = PathBuf::from(trimmed);
    if p.is_absolute() {
        Ok(p)
    } else {
        Ok(workspace_root.join(p))
    }
}

/// Returns `true` if `workspace_root` is inside a git working tree.
pub fn is_git_repo(workspace_root: &Path) -> bool {
    run_git_output(workspace_root, &["rev-parse", "--is-inside-work-tree"])
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Returns the current `HEAD` commit hash, or `None` if the repo has no commits yet.
pub fn head_commit_hash(workspace_root: &Path) -> Result<Option<String>, StorageError> {
    let out = run_git_output(workspace_root, &["rev-parse", "--verify", "HEAD"])?;
    if !out.status.success() {
        return Ok(None);
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() {
        Ok(None)
    } else {
        Ok(Some(hash))
    }
}

/// Ensures `workspace_root` is a git repository (initializing one if needed), configures local
/// exclusions for ephemeral Testrix state files, and creates an initial commit if the repo is empty.
///
/// Returns `Ok(true)` if a new git repository was initialized, or `Ok(false)` if an existing
/// repository was used.
pub fn ensure_workspace_git_repo(workspace_root: &Path) -> Result<bool, StorageError> {
    if !workspace_root.exists() {
        fs::create_dir_all(workspace_root).map_err(|source| StorageError::Io {
            path: workspace_root.to_path_buf(),
            source,
        })?;
    }

    let initialized_new = if is_git_repo(workspace_root) {
        false
    } else {
        debug!(path = %workspace_root.display(), "initializing git repository for workspace");
        let init_b = run_git_output(workspace_root, &["init", "-b", "main"])?;
        if !init_b.status.success() {
            run_git_checked(workspace_root, &["init"])?;
        }
        let gitignore_path = workspace_root.join(".gitignore");
        if !gitignore_path.exists() {
            ensure_lines_in_file(&gitignore_path, LOCAL_IGNORE_PATTERNS)?;
        }
        true
    };

    // Always exclude ephemeral Testrix files via .git/info/exclude so existing repos
    // without those entries in .gitignore never commit local tab/response state.
    if let Ok(git_dir) = resolve_git_dir(workspace_root) {
        let exclude_path = git_dir.join("info").join("exclude");
        let _ = ensure_lines_in_file(&exclude_path, LOCAL_IGNORE_PATTERNS);
    }

    // Create an initial commit if HEAD does not exist yet and there are files to commit.
    if head_commit_hash(workspace_root)?.is_none() {
        let _ = commit_workspace_change(workspace_root, "Initial workspace commit")?;
    }

    Ok(initialized_new)
}

/// Stages all non-ignored workspace changes and creates a git commit with `message` if anything
/// changed. Returns `Ok(Some(commit_hash))` when a commit was created, or `Ok(None)` when the
/// working tree was clean.
pub fn commit_workspace_change(
    workspace_root: &Path,
    message: &str,
) -> Result<Option<String>, StorageError> {
    if !is_git_repo(workspace_root) {
        ensure_workspace_git_repo(workspace_root)?;
    }

    run_git_checked(workspace_root, &["add", "-A"])?;

    let diff_out = run_git_output(workspace_root, &["diff", "--cached", "--quiet"])?;
    if diff_out.status.success() {
        // Exit code 0 means no staged differences.
        return Ok(None);
    }

    let hash = run_git_commit(workspace_root, message)?;
    Ok(Some(hash))
}

/// Computes a git-friendly forward-slash relative path for `file_path` inside `workspace_root`.
pub fn workspace_relative_path(
    workspace_root: &Path,
    file_path: &Path,
) -> Result<String, StorageError> {
    let rel = if let Ok(r) = file_path.strip_prefix(workspace_root) {
        r.to_path_buf()
    } else {
        let canon_root = fs::canonicalize(workspace_root).map_err(|source| StorageError::Io {
            path: workspace_root.to_path_buf(),
            source,
        })?;
        let canon_file = fs::canonicalize(file_path).map_err(|source| StorageError::Io {
            path: file_path.to_path_buf(),
            source,
        })?;
        canon_file
            .strip_prefix(&canon_root)
            .map_err(|_| StorageError::InvalidPath {
                path: file_path.to_path_buf(),
                reason: format!("path is outside workspace `{}`", workspace_root.display()),
            })?
            .to_path_buf()
    };

    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/"))
}

/// Lists git commits that modified `file_path` (following renames), newest first.
pub fn list_file_commits(
    workspace_root: &Path,
    file_path: &Path,
    limit: usize,
) -> Result<Vec<GitCommitInfo>, StorageError> {
    if !is_git_repo(workspace_root) || head_commit_hash(workspace_root)?.is_none() {
        return Ok(Vec::new());
    }

    let rel_path = workspace_relative_path(workspace_root, file_path)?;
    let limit_str = limit.max(1).to_string();
    let format_arg = "--format=COMMIT\x1f%H\x1f%h\x1f%an\x1f%at\x1f%s";
    let output = run_git_checked(
        workspace_root,
        &[
            "log",
            "--follow",
            "-n",
            &limit_str,
            "--name-only",
            format_arg,
            "--",
            &rel_path,
        ],
    )?;

    let mut commits = Vec::new();
    let mut current: Option<GitCommitInfo> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("COMMIT\x1f") {
            if let Some(prev) = current.take() {
                commits.push(prev);
            }
            let parts: Vec<&str> = rest.splitn(5, '\x1f').collect();
            if parts.len() == 5 {
                current = Some(GitCommitInfo {
                    hash: parts[0].to_string(),
                    short_hash: parts[1].to_string(),
                    author: parts[2].to_string(),
                    timestamp: parts[3].parse::<i64>().unwrap_or(0),
                    message: parts[4].to_string(),
                    file_path_at_commit: None,
                });
            }
        } else if !trimmed.is_empty()
            && let Some(ref mut commit) = current
            && commit.file_path_at_commit.is_none()
        {
            commit.file_path_at_commit = Some(trimmed.to_string());
        }
    }

    if let Some(last) = current.take() {
        commits.push(last);
    }

    Ok(commits)
}

/// Reads the UTF-8 content of `rel_path` at `commit_hash`.
pub fn read_file_at_commit(
    workspace_root: &Path,
    rel_path: &str,
    commit_hash: &str,
) -> Result<String, StorageError> {
    let spec = format!("{commit_hash}:{rel_path}");
    run_git_checked(workspace_root, &["show", &spec])
}

/// Restores `target_file_path` to its content at `commit_hash` and commits the restoration.
pub fn restore_file_from_commit(
    workspace_root: &Path,
    target_file_path: &Path,
    commit_hash: &str,
    path_at_commit: Option<&str>,
) -> Result<String, StorageError> {
    let rel_path = match path_at_commit {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => workspace_relative_path(workspace_root, target_file_path)?,
    };

    let content = read_file_at_commit(workspace_root, &rel_path, commit_hash)?;
    if let Some(parent) = target_file_path.parent() {
        fs::create_dir_all(parent).map_err(|source| StorageError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(target_file_path, content.as_bytes()).map_err(|source| StorageError::Io {
        path: target_file_path.to_path_buf(),
        source,
    })?;

    let short = &commit_hash[..commit_hash.len().min(7)];
    let file_name = target_file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("request");
    let _ = commit_workspace_change(workspace_root, &format!("Restore {file_name} to {short}"))?;

    Ok(content)
}

/// Collects all git-tracked UTF-8 files in `workspace_root` (from working tree, excluding
/// ignored/ephemeral files) as `rel_path -> content`.
pub fn collect_workspace_tracked_files(
    workspace_root: &Path,
) -> Result<BTreeMap<String, String>, StorageError> {
    if !is_git_repo(workspace_root) {
        ensure_workspace_git_repo(workspace_root)?;
    }

    // Stage any untracked non-ignored files so ls-files sees the complete current workspace.
    run_git_checked(workspace_root, &["add", "-A"])?;
    let out = run_git_checked(workspace_root, &["ls-files"])?;

    let mut files = BTreeMap::new();
    for line in out.lines() {
        let rel = line.trim();
        if rel.is_empty() || is_ephemeral_rel_path(rel) {
            continue;
        }
        let abs = workspace_root.join(rel);
        if abs.is_file()
            && let Ok(content) = fs::read_to_string(&abs)
        {
            files.insert(rel.to_string(), content);
        }
    }
    Ok(files)
}

fn is_ephemeral_rel_path(rel: &str) -> bool {
    rel == ".testrix-state.yaml"
        || rel == ".testrix-history.yaml"
        || rel.starts_with(".testrix-responses/")
        || rel == ".DS_Store"
        || rel.ends_with("/.DS_Store")
}

/// Performs a 3-way text merge using `git merge-file`.
///
/// Returns `(merged_content, has_conflicts)`.
pub fn merge_file_three_way(
    base: &str,
    local: &str,
    remote: &str,
) -> Result<(String, bool), StorageError> {
    let dir = tempfile::tempdir().map_err(|e| StorageError::Git(format!("tempdir error: {e}")))?;
    let local_p = dir.path().join("local");
    let base_p = dir.path().join("base");
    let remote_p = dir.path().join("remote");

    fs::write(&local_p, local.as_bytes()).map_err(|source| StorageError::Io {
        path: local_p.clone(),
        source,
    })?;
    fs::write(&base_p, base.as_bytes()).map_err(|source| StorageError::Io {
        path: base_p.clone(),
        source,
    })?;
    fs::write(&remote_p, remote.as_bytes()).map_err(|source| StorageError::Io {
        path: remote_p.clone(),
        source,
    })?;

    let output = Command::new("git")
        .args([
            "merge-file",
            "-p",
            "-L",
            "local",
            "-L",
            "base",
            "-L",
            "remote",
        ])
        .arg(&local_p)
        .arg(&base_p)
        .arg(&remote_p)
        .output()
        .map_err(|e| StorageError::Git(format!("failed to invoke git merge-file: {e}")))?;

    let code = output.status.code().unwrap_or(-1);
    if code == 0 {
        Ok((String::from_utf8_lossy(&output.stdout).to_string(), false))
    } else if code > 0 && code < 128 {
        Ok((String::from_utf8_lossy(&output.stdout).to_string(), true))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(StorageError::Git(format!(
            "git merge-file failed (exit {code}): {stderr}"
        )))
    }
}

/// Applies a merged file map and deletions to `workspace_root` and commits the result.
pub fn apply_synced_files_and_commit(
    workspace_root: &Path,
    files: &BTreeMap<String, String>,
    deleted_paths: &[String],
    commit_message: &str,
) -> Result<Option<String>, StorageError> {
    for rel in deleted_paths {
        if is_ephemeral_rel_path(rel) || rel.contains("..") {
            continue;
        }
        let target = workspace_root.join(rel);
        if target.is_file() {
            fs::remove_file(&target).map_err(|source| StorageError::Io {
                path: target.clone(),
                source,
            })?;
        }
    }

    for (rel, content) in files {
        if is_ephemeral_rel_path(rel) || rel.contains("..") {
            continue;
        }
        let target = workspace_root.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| StorageError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let needs_write = match fs::read_to_string(&target) {
            Ok(existing) => existing != *content,
            Err(_) => true,
        };
        if needs_write {
            fs::write(&target, content.as_bytes()).map_err(|source| StorageError::Io {
                path: target,
                source,
            })?;
        }
    }

    commit_workspace_change(workspace_root, commit_message)
}

/// Snapshot of the last-synced file tree and remote commit OID stored inside `.git/testrix-sync/`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncBaseSnapshot {
    pub remote_head_oid: Option<String>,
    pub files: BTreeMap<String, String>,
}

/// Result of a 3-way merge between `base`, `local`, and `remote` file trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreeWayMergeResult {
    pub merged_files: BTreeMap<String, String>,
    pub deleted_local_paths: Vec<String>,
    pub conflicts: Vec<String>,
    pub local_changed: bool,
    pub remote_changed: bool,
}

fn sync_base_file_path(workspace_root: &Path, repo_slug: &str) -> Result<PathBuf, StorageError> {
    let git_dir = resolve_git_dir(workspace_root)?;
    let safe_slug: String = repo_slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = if safe_slug.is_empty() {
        "default".to_string()
    } else {
        safe_slug
    };
    Ok(git_dir.join("testrix-sync").join(format!("{name}.json")))
}

/// Loads the last-synced base snapshot for `repo_slug` in `workspace_root`, if any.
pub fn load_sync_base(
    workspace_root: &Path,
    repo_slug: &str,
) -> Result<Option<SyncBaseSnapshot>, StorageError> {
    if !is_git_repo(workspace_root) {
        return Ok(None);
    }
    let path = sync_base_file_path(workspace_root, repo_slug)?;
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|source| StorageError::Io {
        path: path.clone(),
        source,
    })?;
    let snap: SyncBaseSnapshot = serde_json::from_str(&raw)
        .map_err(|e| StorageError::Git(format!("invalid sync base snapshot: {e}")))?;
    Ok(Some(snap))
}

/// Saves the last-synced base snapshot for `repo_slug` in `workspace_root`.
pub fn save_sync_base(
    workspace_root: &Path,
    repo_slug: &str,
    snapshot: &SyncBaseSnapshot,
) -> Result<(), StorageError> {
    if !is_git_repo(workspace_root) {
        ensure_workspace_git_repo(workspace_root)?;
    }
    let path = sync_base_file_path(workspace_root, repo_slug)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| StorageError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|e| StorageError::Git(format!("failed to serialize sync base: {e}")))?;
    fs::write(&path, json.as_bytes()).map_err(|source| StorageError::Io { path, source })
}

/// Performs a 3-way merge across `base`, `local`, and `remote` file trees.
pub fn three_way_merge_trees(
    base: &BTreeMap<String, String>,
    local: &BTreeMap<String, String>,
    remote: &BTreeMap<String, String>,
) -> Result<ThreeWayMergeResult, StorageError> {
    let mut all_paths = std::collections::BTreeSet::new();
    all_paths.extend(base.keys().cloned());
    all_paths.extend(local.keys().cloned());
    all_paths.extend(remote.keys().cloned());

    let mut merged_files = BTreeMap::new();
    let mut deleted_local_paths = Vec::new();
    let mut conflicts = Vec::new();
    let mut local_changed = false;
    let mut remote_changed = false;

    for path in all_paths {
        let b = base.get(&path);
        let l = local.get(&path);
        let r = remote.get(&path);

        if l == r {
            if let Some(content) = l {
                merged_files.insert(path, content.clone());
            }
            continue;
        }

        if b == l {
            // Unchanged locally, modified or deleted remotely.
            local_changed = true;
            if let Some(content) = r {
                merged_files.insert(path, content.clone());
            } else {
                deleted_local_paths.push(path);
            }
            continue;
        }

        if b == r {
            // Unchanged remotely, modified or deleted locally.
            remote_changed = true;
            if let Some(content) = l {
                merged_files.insert(path, content.clone());
            }
            continue;
        }

        // Both sides changed this path differently.
        local_changed = true;
        remote_changed = true;
        match (l, r) {
            (Some(lc), Some(rc)) => {
                let bc = b.map(String::as_str).unwrap_or("");
                let (merged, has_conflict) = merge_file_three_way(bc, lc, rc)?;
                merged_files.insert(path.clone(), merged);
                if has_conflict {
                    conflicts.push(path);
                }
            }
            (Some(lc), None) => {
                // Local modified, remote deleted: keep local and flag conflict.
                merged_files.insert(path.clone(), lc.clone());
                conflicts.push(path);
            }
            (None, Some(rc)) => {
                // Local deleted, remote modified: restore remote and flag conflict.
                merged_files.insert(path.clone(), rc.clone());
                conflicts.push(path);
            }
            (None, None) => {}
        }
    }

    Ok(ThreeWayMergeResult {
        merged_files,
        deleted_local_paths,
        conflicts,
        local_changed,
        remote_changed,
    })
}
