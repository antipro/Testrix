use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
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

fn git_err(context: &str, err: impl std::fmt::Display) -> StorageError {
    StorageError::Git(format!("{context}: {err}"))
}

fn open_repo(workspace_root: &Path) -> Result<gix::Repository, StorageError> {
    gix::discover(workspace_root).map_err(|e| git_err("failed to open git repository", e))
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

/// Returns `true` if `workspace_root` is inside a git working tree.
pub fn is_git_repo(workspace_root: &Path) -> bool {
    gix::discover(workspace_root).is_ok()
}

/// Returns the current `HEAD` commit hash, or `None` if the repo has no commits yet.
pub fn head_commit_hash(workspace_root: &Path) -> Result<Option<String>, StorageError> {
    let repo = open_repo(workspace_root)?;
    match repo.head_id() {
        Ok(id) => Ok(Some(id.to_string())),
        Err(_) => Ok(None),
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

    let (repo, initialized_new) = match gix::discover(workspace_root) {
        Ok(repo) => (repo, false),
        Err(_) => {
            debug!(path = %workspace_root.display(), "initializing pure-rust git repository for workspace");
            let repo = gix::init(workspace_root).map_err(|e| git_err("git init failed", e))?;
            let gitignore_path = workspace_root.join(".gitignore");
            if !gitignore_path.exists() {
                ensure_lines_in_file(&gitignore_path, LOCAL_IGNORE_PATTERNS)?;
            }
            (repo, true)
        }
    };

    // Always exclude ephemeral Testrix files via .git/info/exclude so existing repos
    // without those entries in .gitignore never commit local tab/response state.
    let exclude_path = repo.git_dir().join("info").join("exclude");
    let _ = ensure_lines_in_file(&exclude_path, LOCAL_IGNORE_PATTERNS);

    // Create an initial commit if HEAD does not exist yet and there are files to commit.
    if head_commit_hash(workspace_root)?.is_none() {
        let _ = commit_workspace_change(workspace_root, "Initial workspace commit")?;
    }

    Ok(initialized_new)
}

fn collect_workspace_files_raw(
    work_dir: &Path,
    cur_dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), StorageError> {
    let entries = fs::read_dir(cur_dir).map_err(|source| StorageError::Io {
        path: cur_dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| StorageError::Io {
            path: cur_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if name == ".git" || name == ".DS_Store" || name.contains(".tmp.") {
            continue;
        }
        let rel = workspace_relative_path(work_dir, &path)?;
        if is_ephemeral_rel_path(&rel) {
            continue;
        }
        if path.is_dir() {
            collect_workspace_files_raw(work_dir, &path, out)?;
        } else if path.is_file() {
            let bytes = fs::read(&path).map_err(|source| StorageError::Io { path, source })?;
            out.insert(rel, bytes);
        }
    }
    Ok(())
}

fn sync_git_index_to_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
) -> Result<(), StorageError> {
    let state = gix::index::State::from_tree(&tree_id, &repo.objects, Default::default())
        .map_err(|e| git_err("failed building index from tree", e))?;
    let mut index = gix::index::File::from_state(state, repo.index_path());
    index
        .write(Default::default(), Default::default())
        .map_err(|e| git_err("failed writing git index", e))?;
    Ok(())
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

    let mut repo = open_repo(workspace_root)?;
    let work_dir = repo
        .workdir()
        .unwrap_or(workspace_root)
        .to_path_buf();

    let mut files = BTreeMap::new();
    collect_workspace_files_raw(&work_dir, &work_dir, &mut files)?;

    let empty_tree_id = repo.empty_tree().id;
    let new_tree_id = {
        let mut editor = repo
            .edit_tree(empty_tree_id)
            .map_err(|e| git_err("failed creating tree editor", e))?;
        for (rel, bytes) in &files {
            let blob_id = repo
                .write_blob(bytes)
                .map_err(|e| git_err("failed writing blob", e))?;
            editor
                .upsert(rel.as_str(), gix::object::tree::EntryKind::Blob, blob_id)
                .map_err(|e| git_err("failed upserting tree entry", e))?;
        }
        editor
            .write()
            .map_err(|e| git_err("failed writing tree", e))?
            .detach()
    };

    if let Ok(head_tree_id) = repo.head_tree_id()
        && head_tree_id.detach() == new_tree_id
    {
        return Ok(None);
    }

    let parents: Vec<gix::ObjectId> = repo
        .head_id()
        .ok()
        .map(|id| vec![id.detach()])
        .unwrap_or_default();

    // Ensure committer/author identity exists (falling back to Testrix <testrix@local>).
    let sig_owned = match repo.committer() {
        Some(Ok(sig)) => sig.to_owned().map_err(|e| git_err("invalid committer", e))?,
        _ => {
            repo.committer_or_set_generic_fallback()
                .map_err(|e| git_err("failed setting fallback identity", e))?;
            let time = gix::date::Time::now_local_or_utc();
            gix::actor::Signature {
                name: "Testrix".into(),
                email: "testrix@local".into(),
                time,
            }
        }
    };
    let mut time_buf = gix::date::parse::TimeBuf::default();
    let sig_ref = sig_owned.to_ref(&mut time_buf);

    let commit_id = repo
        .commit_as(sig_ref, sig_ref, "HEAD", message, new_tree_id, parents)
        .map_err(|e| git_err("failed creating commit", e))?;

    let _ = sync_git_index_to_tree(&repo, new_tree_id);

    Ok(Some(commit_id.to_string()))
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

fn tree_blob_id_at_path(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    rel_path: &str,
) -> Option<gix::ObjectId> {
    let tree = repo.find_tree(tree_id).ok()?;
    let entry = tree.lookup_entry_by_path(rel_path).ok()??;
    Some(entry.object_id())
}

/// Lists git commits that modified `file_path`, newest first.
pub fn list_file_commits(
    workspace_root: &Path,
    file_path: &Path,
    limit: usize,
) -> Result<Vec<GitCommitInfo>, StorageError> {
    if !is_git_repo(workspace_root) {
        return Ok(Vec::new());
    }
    let repo = open_repo(workspace_root)?;
    let Ok(head_id) = repo.head_id() else {
        return Ok(Vec::new());
    };

    let rel_path = workspace_relative_path(workspace_root, file_path)?;
    let max_commits = limit.max(1);
    let mut commits = Vec::new();

    let walk = repo
        .rev_walk([head_id.detach()])
        .all()
        .map_err(|e| git_err("failed starting rev walk", e))?;

    for item in walk {
        let info = item.map_err(|e| git_err("rev walk error", e))?;
        let commit = info
            .object()
            .map_err(|e| git_err("failed loading commit", e))?;
        let tree_id = commit
            .tree_id()
            .map_err(|e| git_err("failed reading commit tree", e))?
            .detach();

        let cur_blob = tree_blob_id_at_path(&repo, tree_id, &rel_path);
        let parent_id = commit.parent_ids().next().map(|p| p.detach());
        let parent_blob = parent_id.and_then(|pid| {
            let pcommit = repo.find_commit(pid).ok()?;
            let ptree = pcommit.tree_id().ok()?.detach();
            tree_blob_id_at_path(&repo, ptree, &rel_path)
        });

        // Include commit if the file was added or modified in this commit.
        if cur_blob.is_some() && cur_blob != parent_blob {
            let hash = commit.id.to_string();
            let short_hash = hash[..hash.len().min(7)].to_string();
            let author_sig = commit
                .author()
                .map_err(|e| git_err("failed reading commit author", e))?;
            let author = author_sig.name.to_string();
            let timestamp = commit.time().map(|t| t.seconds).unwrap_or(0);
            let message = commit
                .message_raw_sloppy()
                .to_string()
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();

            commits.push(GitCommitInfo {
                hash,
                short_hash,
                author,
                timestamp,
                message,
                file_path_at_commit: Some(rel_path.clone()),
            });

            if commits.len() >= max_commits {
                break;
            }
        }
    }

    Ok(commits)
}

/// Reads the UTF-8 content of `rel_path` at `commit_hash`.
pub fn read_file_at_commit(
    workspace_root: &Path,
    rel_path: &str,
    commit_hash: &str,
) -> Result<String, StorageError> {
    let repo = open_repo(workspace_root)?;
    let oid = gix::ObjectId::from_hex(commit_hash.as_bytes())
        .map_err(|e| git_err("invalid commit hash", e))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| git_err("commit not found", e))?;
    let tree = commit
        .tree()
        .map_err(|e| git_err("failed loading commit tree", e))?;
    let entry = tree
        .lookup_entry_by_path(rel_path)
        .map_err(|e| git_err("failed looking up path in tree", e))?
        .ok_or_else(|| StorageError::Git(format!("path `{rel_path}` not found in {commit_hash}")))?;
    let obj = entry
        .object()
        .map_err(|e| git_err("failed reading blob object", e))?;
    Ok(String::from_utf8_lossy(&obj.data).to_string())
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

    let mut raw_files = BTreeMap::new();
    collect_workspace_files_raw(workspace_root, workspace_root, &mut raw_files)?;

    let mut files = BTreeMap::new();
    for (rel, bytes) in raw_files {
        if let Ok(text) = String::from_utf8(bytes) {
            files.insert(rel, text);
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

/// Performs an in-memory pure-Rust 3-way text merge using `gix::merge`.
///
/// Returns `(merged_content, has_conflicts)`.
pub fn merge_file_three_way(
    base: &str,
    local: &str,
    remote: &str,
) -> Result<(String, bool), StorageError> {
    use gix::merge::blob::{
        Resolution,
        builtin_driver::{
            text,
            text::{Labels, Options},
        },
    };

    let mut out = Vec::new();
    let mut input = gix::diff::blob::InternedInput::new(base.as_bytes(), local.as_bytes());
    let labels = Labels {
        ancestor: Some("base".into()),
        current: Some("local".into()),
        other: Some("remote".into()),
    };

    let resolution = text(
        &mut out,
        &mut input,
        labels,
        local.as_bytes(),
        base.as_bytes(),
        remote.as_bytes(),
        Options::default(),
    );

    let has_conflicts = matches!(resolution, Resolution::Conflict);
    Ok((String::from_utf8_lossy(&out).to_string(), has_conflicts))
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
    let repo = open_repo(workspace_root)?;
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
    Ok(repo.git_dir().join("testrix-sync").join(format!("{name}.json")))
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
        match (l, r) {
            (Some(lc), Some(rc)) => {
                let bc = b.map(String::as_str).unwrap_or("");
                if path.ends_with(".env.yaml")
                    && let Some(merged_env) = merge_environment_yaml_three_way(bc, lc, rc)
                {
                    if merged_env != *lc {
                        local_changed = true;
                    }
                    if merged_env != *rc {
                        remote_changed = true;
                    }
                    merged_files.insert(path, merged_env);
                    continue;
                }
                let (merged, has_conflict) = merge_file_three_way(bc, lc, rc)?;
                let is_yaml = path.ends_with(".yaml") || path.ends_with(".yml");
                if has_conflict && is_yaml {
                    // Never write Git conflict markers (`<<<<<<<`) into structured YAML files,
                    // because it breaks YAML workspace parsing. Keep a valid YAML side (prefer
                    // local unless local already contains conflict markers) and report the conflict.
                    let chosen = if lc.contains("<<<<<<<") && !rc.contains("<<<<<<<") {
                        local_changed = true;
                        rc.clone()
                    } else {
                        remote_changed = true;
                        lc.clone()
                    };
                    merged_files.insert(path.clone(), chosen);
                    conflicts.push(path);
                } else {
                    local_changed = true;
                    remote_changed = true;
                    merged_files.insert(path.clone(), merged);
                    if has_conflict {
                        conflicts.push(path);
                    }
                }
            }
            (Some(lc), None) => {
                // Local modified, remote deleted: keep local and flag conflict.
                local_changed = true;
                remote_changed = true;
                merged_files.insert(path.clone(), lc.clone());
                conflicts.push(path);
            }
            (None, Some(rc)) => {
                // Local deleted, remote modified: restore remote and flag conflict.
                local_changed = true;
                remote_changed = true;
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

fn merge_environment_yaml_three_way(
    base_yaml: &str,
    local_yaml: &str,
    remote_yaml: &str,
) -> Option<String> {
    let local_doc: testrix_core::EnvironmentDoc = yaml_serde::from_str(local_yaml).ok()?;
    let remote_doc: testrix_core::EnvironmentDoc = yaml_serde::from_str(remote_yaml).ok()?;
    let base_doc: Option<testrix_core::EnvironmentDoc> = if base_yaml.trim().is_empty() {
        None
    } else {
        yaml_serde::from_str(base_yaml).ok()
    };

    let local_map: BTreeMap<String, testrix_core::EnvVariable> = local_doc
        .variables
        .iter()
        .map(|v| (v.key.clone(), v.clone()))
        .collect();
    let remote_map: BTreeMap<String, testrix_core::EnvVariable> = remote_doc
        .variables
        .iter()
        .map(|v| (v.key.clone(), v.clone()))
        .collect();

    let mut merged_vars: Vec<testrix_core::EnvVariable> = Vec::new();
    let mut seen_keys = std::collections::BTreeSet::new();

    if let Some(base) = base_doc {
        let base_map: BTreeMap<String, testrix_core::EnvVariable> = base
            .variables
            .iter()
            .map(|v| (v.key.clone(), v.clone()))
            .collect();

        let mut all_keys = Vec::new();
        for v in remote_doc
            .variables
            .iter()
            .chain(local_doc.variables.iter())
            .chain(base.variables.iter())
        {
            if seen_keys.insert(v.key.clone()) {
                all_keys.push(v.key.clone());
            }
        }

        for k in all_keys {
            let b = base_map.get(&k);
            let l = local_map.get(&k);
            let r = remote_map.get(&k);
            let chosen = if l == r {
                l.cloned()
            } else if b == l {
                r.cloned()
            } else if b == r {
                l.cloned()
            } else {
                // Both modified: prefer non-empty local, fallback to remote.
                match (l, r) {
                    (Some(lv), Some(rv)) => {
                        if lv.value.trim().is_empty() && !rv.value.trim().is_empty() {
                            Some(rv.clone())
                        } else {
                            Some(lv.clone())
                        }
                    }
                    (Some(lv), None) => Some(lv.clone()),
                    (None, Some(rv)) => Some(rv.clone()),
                    (None, None) => None,
                }
            };
            if let Some(var) = chosen {
                merged_vars.push(var);
            }
        }
    } else {
        // No common base (e.g. fresh workspace syncing with existing remote):
        // preserve all remote variables and add any extra local variables.
        for rv in &remote_doc.variables {
            seen_keys.insert(rv.key.clone());
            if let Some(lv) = local_map.get(&rv.key)
                && rv.value.trim().is_empty()
                && !lv.value.trim().is_empty()
            {
                merged_vars.push(lv.clone());
            } else {
                merged_vars.push(rv.clone());
            }
        }
        for lv in &local_doc.variables {
            if seen_keys.insert(lv.key.clone()) {
                merged_vars.push(lv.clone());
            }
        }
    }

    let mut merged_doc = remote_doc;
    if merged_doc.name.trim().is_empty() {
        merged_doc.name = local_doc.name;
    }
    if merged_doc.description.trim().is_empty() {
        merged_doc.description = local_doc.description;
    }
    merged_doc.variables = merged_vars;

    yaml_serde::to_string(&merged_doc).ok()
}
