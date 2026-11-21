use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use testrix_core::{
    DocumentKind, EnvironmentDoc, EnvironmentNode, FolderDoc, FolderNode, HttpMethod,
    HttpRequestDoc, HttpResponseSnapshot, ImportedProjectBundle, ProjectDoc, ProjectNode,
    RequestHistoryEntry, RequestNode, RequestSet, WorkspaceTree, generate_id,
};
use tracing::{debug, warn};

use crate::{
    error::StorageError,
    seed::seed_starter_workspace,
    slug::{unique_dir_path, unique_file_path},
};

const PROJECT_MANIFEST_FILE: &str = "testrix.project.yaml";
const FOLDER_MANIFEST_FILE: &str = "_folder.yaml";
const ENVIRONMENTS_DIR: &str = "environments";
const REQUEST_EXTENSION: &str = ".request.yaml";
const ENV_EXTENSION: &str = ".env.yaml";
const WORKSPACE_STATE_FILE: &str = ".testrix-state.yaml";
const WORKSPACE_HISTORY_FILE: &str = ".testrix-history.yaml";
const WORKSPACE_SETS_FILE: &str = ".testrix-sets.yaml";
const RESPONSES_CACHE_DIR: &str = ".testrix-responses";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceStateFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_environment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    open_tabs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_tab_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    expanded_nodes: Vec<String>,
}

/// Persisted open-tab list and active tab key for a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceTabState {
    pub open_tabs: Vec<String>,
    pub active_tab_key: Option<String>,
}

/// Serializes any Testrix YAML document to canonical pretty YAML.
pub fn to_yaml_pretty<T: Serialize>(doc: &T) -> Result<String, StorageError> {
    Ok(yaml_serde::to_string(doc)?)
}

/// Atomically writes a YAML-serializable document to `path`.
pub fn write_yaml_atomic<T: Serialize>(path: &Path, doc: &T) -> Result<(), StorageError> {
    let yaml = to_yaml_pretty(doc)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| StorageError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let file_name =
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| StorageError::InvalidPath {
                path: path.to_path_buf(),
                reason: "missing filename".to_string(),
            })?;

    let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = path.with_file_name(format!(".{file_name}.tmp.{}.{nonce}", std::process::id()));

    std::fs::write(&tmp_path, yaml.as_bytes()).map_err(|source| StorageError::Io {
        path: tmp_path.clone(),
        source,
    })?;

    std::fs::rename(&tmp_path, path).map_err(|source| {
        let _ = std::fs::remove_file(&tmp_path);
        StorageError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;

    Ok(())
}

/// Ensures `workspace_root` exists and contains at least one project (seeding starter data if empty),
/// then loads the entire workspace tree.
pub fn ensure_seeded_workspace(workspace_root: &Path) -> Result<WorkspaceTree, StorageError> {
    std::fs::create_dir_all(workspace_root).map_err(|source| StorageError::Io {
        path: workspace_root.to_path_buf(),
        source,
    })?;

    let mut tree = load_workspace(workspace_root)?;
    if tree.projects.is_empty() {
        debug!(path = %workspace_root.display(), "seeding starter Testrix workspace");
        seed_starter_workspace(workspace_root)?;
        tree = load_workspace(workspace_root)?;
        if let Some(first_project) = tree.projects.first() {
            tree.active_project_id = Some(first_project.doc.id.clone());
            tree.active_environment_id = tree
                .environments
                .iter()
                .find(|env| env.doc.name.eq_ignore_ascii_case("Development"))
                .or_else(|| tree.environments.first())
                .map(|env| env.doc.id.clone());
            let _ = save_workspace_state(
                workspace_root,
                tree.active_project_id.as_deref(),
                tree.active_environment_id.as_deref(),
            );
        }
    }

    Ok(tree)
}

/// Loads all projects, folders, HTTP requests, and shared environments from `workspace_root`.
pub fn load_workspace(workspace_root: &Path) -> Result<WorkspaceTree, StorageError> {
    if !workspace_root.exists() {
        return Ok(WorkspaceTree {
            root_dir: workspace_root.to_path_buf(),
            projects: Vec::new(),
            environments: Vec::new(),
            active_project_id: None,
            active_environment_id: None,
        });
    }

    let mut projects = Vec::new();
    let root_manifest = workspace_root.join(PROJECT_MANIFEST_FILE);
    if root_manifest.is_file() {
        projects.push(load_project(workspace_root)?);
    } else {
        let entries = std::fs::read_dir(workspace_root).map_err(|source| StorageError::Io {
            path: workspace_root.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| StorageError::Io {
                path: workspace_root.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if !path.is_dir() || is_hidden_name(&path) {
                continue;
            }
            if path.join(PROJECT_MANIFEST_FILE).is_file() {
                projects.push(load_project(&path)?);
            }
        }
    }

    projects.sort_by_key(|a| a.doc.name.to_lowercase());

    // Load workspace-wide shared profiles from `<workspace_root>/environments/*.env.yaml`.
    let ws_env_dir = workspace_root.join(ENVIRONMENTS_DIR);
    let mut environments = if ws_env_dir.is_dir() {
        load_environments(&ws_env_dir)?
    } else {
        Vec::new()
    };

    // Migrate any legacy per-project `<project_dir>/environments/*.env.yaml` into workspace root.
    if !root_manifest.is_file() {
        let mut migrated_any = false;
        for proj in &mut projects {
            if proj.environments.is_empty() {
                continue;
            }
            let docs: Vec<EnvironmentDoc> =
                proj.environments.iter().map(|e| e.doc.clone()).collect();
            merge_environments_into_dir(&ws_env_dir, &docs)?;
            for legacy in proj.environments.drain(..) {
                let _ = std::fs::remove_file(&legacy.file_path);
            }
            let legacy_dir = proj.root_dir.join(ENVIRONMENTS_DIR);
            if legacy_dir.is_dir() {
                let _ = std::fs::remove_dir(&legacy_dir);
            }
            migrated_any = true;
        }
        if migrated_any && ws_env_dir.is_dir() {
            environments = load_environments(&ws_env_dir)?;
        }
    }

    environments.sort_by_key(|a| a.doc.name.to_lowercase());

    let state_path = workspace_root.join(WORKSPACE_STATE_FILE);
    let state = if state_path.is_file() {
        read_yaml_file::<WorkspaceStateFile>(&state_path).unwrap_or_default()
    } else {
        WorkspaceStateFile::default()
    };

    let active_project_id = state
        .active_project_id
        .filter(|id| projects.iter().any(|p| &p.doc.id == id))
        .or_else(|| projects.first().map(|p| p.doc.id.clone()));

    let active_environment_id = state
        .active_environment_id
        .filter(|env_id| environments.iter().any(|e| &e.doc.id == env_id));

    Ok(WorkspaceTree {
        root_dir: workspace_root.to_path_buf(),
        projects,
        environments,
        active_project_id,
        active_environment_id,
    })
}

/// Persists active project/environment UI selection in `.testrix-state.yaml`,
/// preserving any previously saved open-tab state.
pub fn save_workspace_state(
    workspace_root: &Path,
    active_project_id: Option<&str>,
    active_environment_id: Option<&str>,
) -> Result<(), StorageError> {
    let state_path = workspace_root.join(WORKSPACE_STATE_FILE);
    let mut state = if state_path.is_file() {
        read_yaml_file::<WorkspaceStateFile>(&state_path).unwrap_or_default()
    } else {
        WorkspaceStateFile::default()
    };
    state.active_project_id = active_project_id.map(str::to_string);
    state.active_environment_id = active_environment_id.map(str::to_string);
    write_yaml_atomic(&state_path, &state)
}

/// Loads the persisted open-tab keys and active tab key from `.testrix-state.yaml`.
pub fn load_workspace_tabs(workspace_root: &Path) -> WorkspaceTabState {
    let state_path = workspace_root.join(WORKSPACE_STATE_FILE);
    if !state_path.is_file() {
        return WorkspaceTabState::default();
    }
    let state = read_yaml_file::<WorkspaceStateFile>(&state_path).unwrap_or_default();
    WorkspaceTabState {
        open_tabs: state.open_tabs,
        active_tab_key: state.active_tab_key,
    }
}

/// Loads the persisted expanded explorer node IDs from `.testrix-state.yaml`.
pub fn load_workspace_explorer_state(workspace_root: &Path) -> Vec<String> {
    let state_path = workspace_root.join(WORKSPACE_STATE_FILE);
    if !state_path.is_file() {
        return Vec::new();
    }
    read_yaml_file::<WorkspaceStateFile>(&state_path)
        .map(|s| s.expanded_nodes)
        .unwrap_or_default()
}

/// Persists the expanded explorer node IDs in `.testrix-state.yaml`,
/// preserving active project/environment and open-tab state.
pub fn save_workspace_explorer_state(
    workspace_root: &Path,
    expanded_nodes: &[String],
) -> Result<(), StorageError> {
    let state_path = workspace_root.join(WORKSPACE_STATE_FILE);
    let mut state = if state_path.is_file() {
        read_yaml_file::<WorkspaceStateFile>(&state_path).unwrap_or_default()
    } else {
        WorkspaceStateFile::default()
    };
    state.expanded_nodes = expanded_nodes.to_vec();
    write_yaml_atomic(&state_path, &state)
}

/// Persists the open-tab keys and active tab key into `.testrix-state.yaml`.
pub fn save_workspace_tabs(
    workspace_root: &Path,
    open_tabs: &[String],
    active_tab_key: Option<&str>,
) -> Result<(), StorageError> {
    let state_path = workspace_root.join(WORKSPACE_STATE_FILE);
    let mut state = if state_path.is_file() {
        read_yaml_file::<WorkspaceStateFile>(&state_path).unwrap_or_default()
    } else {
        WorkspaceStateFile::default()
    };
    state.open_tabs = open_tabs.to_vec();
    state.active_tab_key = active_tab_key.map(str::to_string);
    write_yaml_atomic(&state_path, &state)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceHistoryFile {
    #[serde(default)]
    entries: Vec<RequestHistoryEntry>,
}

/// Loads persisted HTTP request execution history from `.testrix-history.yaml`.
pub fn load_workspace_history(workspace_root: &Path) -> Vec<RequestHistoryEntry> {
    let path = workspace_root.join(WORKSPACE_HISTORY_FILE);
    if !path.is_file() {
        return Vec::new();
    }
    read_yaml_file::<WorkspaceHistoryFile>(&path)
        .map(|f| f.entries)
        .unwrap_or_default()
}

/// Persists HTTP request execution history into `.testrix-history.yaml`.
pub fn save_workspace_history(
    workspace_root: &Path,
    entries: &[RequestHistoryEntry],
) -> Result<(), StorageError> {
    let path = workspace_root.join(WORKSPACE_HISTORY_FILE);
    let file = WorkspaceHistoryFile {
        entries: entries.to_vec(),
    };
    write_yaml_atomic(&path, &file)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSetsFile {
    #[serde(default)]
    sets: Vec<RequestSet>,
}

/// Loads persisted request sets from `.testrix-sets.yaml`.
pub fn load_workspace_sets(workspace_root: &Path) -> Vec<RequestSet> {
    let path = workspace_root.join(WORKSPACE_SETS_FILE);
    if !path.is_file() {
        return Vec::new();
    }
    read_yaml_file::<WorkspaceSetsFile>(&path)
        .map(|f| f.sets)
        .unwrap_or_default()
}

/// Persists request sets into `.testrix-sets.yaml`.
pub fn save_workspace_sets(
    workspace_root: &Path,
    sets: &[RequestSet],
) -> Result<(), StorageError> {
    let path = workspace_root.join(WORKSPACE_SETS_FILE);
    let file = WorkspaceSetsFile {
        sets: sets.to_vec(),
    };
    write_yaml_atomic(&path, &file)
}

fn response_cache_path(workspace_root: &Path, request_id: &str) -> PathBuf {
    let safe_id = crate::slug::slugify(request_id);
    workspace_root
        .join(RESPONSES_CACHE_DIR)
        .join(format!("{safe_id}.yaml"))
}

/// Persists the last HTTP response snapshot for `request_id` under `.testrix-responses/`.
pub fn save_request_response(
    workspace_root: &Path,
    request_id: &str,
    snapshot: &HttpResponseSnapshot,
) -> Result<(), StorageError> {
    write_yaml_atomic(&response_cache_path(workspace_root, request_id), snapshot)
}

/// Loads the last persisted HTTP response snapshot for `request_id`, if present.
pub fn load_request_response(
    workspace_root: &Path,
    request_id: &str,
) -> Option<HttpResponseSnapshot> {
    let path = response_cache_path(workspace_root, request_id);
    if !path.is_file() {
        return None;
    }
    read_yaml_file::<HttpResponseSnapshot>(&path).ok()
}

/// Removes all persisted HTTP response snapshots under `.testrix-responses/`.
pub fn clear_saved_responses(workspace_root: &Path) -> Result<(), StorageError> {
    let dir = workspace_root.join(RESPONSES_CACHE_DIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|source| StorageError::Io { path: dir, source })?;
    }
    Ok(())
}

/// Loads a single project directory.
pub fn load_project(project_dir: &Path) -> Result<ProjectNode, StorageError> {
    let manifest_path = project_dir.join(PROJECT_MANIFEST_FILE);
    let doc: ProjectDoc = read_yaml_file(&manifest_path)?;
    doc.validate()?;

    let mut folders = Vec::new();
    let mut requests = Vec::new();
    let mut environments = Vec::new();

    let entries = std::fs::read_dir(project_dir).map_err(|source| StorageError::Io {
        path: project_dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| StorageError::Io {
            path: project_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if is_hidden_name(&path) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if path.is_dir() {
            if name == ENVIRONMENTS_DIR {
                environments = load_environments(&path)?;
            } else {
                folders.push(load_folder(project_dir, &path)?);
            }
        } else if path.is_file() && name.ends_with(REQUEST_EXTENSION) {
            requests.push(load_request(&path)?);
        }
    }

    sort_folders(&mut folders);
    sort_requests(&mut requests);
    environments.sort_by_key(|a| a.doc.name.to_lowercase());

    Ok(ProjectNode {
        doc,
        root_dir: project_dir.to_path_buf(),
        manifest_path,
        folders,
        requests,
        environments,
    })
}

fn load_environments(env_dir: &Path) -> Result<Vec<EnvironmentNode>, StorageError> {
    let mut envs = Vec::new();
    let entries = std::fs::read_dir(env_dir).map_err(|source| StorageError::Io {
        path: env_dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| StorageError::Io {
            path: env_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() || is_hidden_name(&path) {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.ends_with(ENV_EXTENSION))
        {
            let mut doc: EnvironmentDoc = read_yaml_file(&path)?;
            doc.validate()?;
            testrix_core::sort_env_variables(&mut doc.variables);
            envs.push(EnvironmentNode {
                doc,
                file_path: path,
            });
        }
    }
    Ok(envs)
}

fn load_folder(project_root: &Path, dir_path: &Path) -> Result<FolderNode, StorageError> {
    let manifest_path = dir_path.join(FOLDER_MANIFEST_FILE);
    let (doc, has_manifest) = if manifest_path.is_file() {
        let parsed: FolderDoc = read_yaml_file(&manifest_path)?;
        parsed.validate()?;
        (parsed, true)
    } else {
        let rel = dir_path
            .strip_prefix(project_root)
            .unwrap_or(dir_path)
            .to_string_lossy()
            .replace(['\\', '/'], "-");
        let display_name = dir_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Folder");
        (FolderDoc::new(format!("fld-{rel}"), display_name), false)
    };

    let mut folders = Vec::new();
    let mut requests = Vec::new();
    let entries = std::fs::read_dir(dir_path).map_err(|source| StorageError::Io {
        path: dir_path.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| StorageError::Io {
            path: dir_path.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if is_hidden_name(&path) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if path.is_dir() {
            folders.push(load_folder(project_root, &path)?);
        } else if path.is_file() && name.ends_with(REQUEST_EXTENSION) {
            requests.push(load_request(&path)?);
        }
    }

    sort_folders(&mut folders);
    sort_requests(&mut requests);

    Ok(FolderNode {
        doc,
        dir_path: dir_path.to_path_buf(),
        manifest_path,
        has_manifest,
        folders,
        requests,
    })
}

fn load_request(path: &Path) -> Result<RequestNode, StorageError> {
    let doc: HttpRequestDoc = read_yaml_file(path)?;
    doc.validate()?;
    Ok(RequestNode {
        doc,
        file_path: path.to_path_buf(),
    })
}

/// Validates a single Testrix YAML document on disk according to its filename role.
pub fn validate_yaml_file(path: &Path) -> Result<DocumentKind, StorageError> {
    if !path.is_file() {
        return Err(StorageError::InvalidPath {
            path: path.to_path_buf(),
            reason: "path does not exist or is not a file".to_string(),
        });
    }
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return Err(StorageError::InvalidPath {
            path: path.to_path_buf(),
            reason: "invalid UTF-8 filename".to_string(),
        });
    };

    if file_name == PROJECT_MANIFEST_FILE {
        let doc: ProjectDoc = read_yaml_file(path)?;
        doc.validate()?;
        Ok(DocumentKind::Project)
    } else if file_name == FOLDER_MANIFEST_FILE {
        let doc: FolderDoc = read_yaml_file(path)?;
        doc.validate()?;
        Ok(DocumentKind::Folder)
    } else if file_name.ends_with(REQUEST_EXTENSION) {
        let doc: HttpRequestDoc = read_yaml_file(path)?;
        doc.validate()?;
        Ok(DocumentKind::HttpRequest)
    } else if file_name.ends_with(ENV_EXTENSION) {
        let doc: EnvironmentDoc = read_yaml_file(path)?;
        doc.validate()?;
        Ok(DocumentKind::Environment)
    } else {
        Err(StorageError::InvalidPath {
            path: path.to_path_buf(),
            reason: format!(
                "unsupported filename `{file_name}`; expected `{PROJECT_MANIFEST_FILE}`, `{FOLDER_MANIFEST_FILE}`, `*{REQUEST_EXTENSION}`, or `*{ENV_EXTENSION}`"
            ),
        })
    }
}

/// Saves a project manifest (`testrix.project.yaml`).
pub fn save_project(project: &ProjectNode) -> Result<(), StorageError> {
    project.doc.validate()?;
    write_yaml_atomic(&project.manifest_path, &project.doc)
}

/// Creates a new project directory with `testrix.project.yaml` and a default `Development` environment.
pub fn create_project(
    workspace_root: &Path,
    name: &str,
    base_url: &str,
) -> Result<ProjectNode, StorageError> {
    let trimmed = name.trim();
    let project_name = if trimmed.is_empty() {
        "New Project"
    } else {
        trimmed
    };
    let project_id = generate_id("proj");
    let root_dir = unique_dir_path(workspace_root, &project_id);
    std::fs::create_dir_all(&root_dir).map_err(|source| StorageError::Io {
        path: root_dir.clone(),
        source,
    })?;

    let mut project = ProjectDoc::new(project_id, project_name);
    project.base_url = base_url.to_string();
    let manifest_path = root_dir.join(PROJECT_MANIFEST_FILE);
    write_yaml_atomic(&manifest_path, &project)?;

    load_project(&root_dir)
}

fn persist_imported_folder(
    parent_dir: &Path,
    folder: &testrix_core::ImportedFolder,
) -> Result<(), StorageError> {
    folder.doc.validate()?;
    let folder_dir = unique_dir_path(parent_dir, &folder.doc.name);
    std::fs::create_dir_all(&folder_dir).map_err(|source| StorageError::Io {
        path: folder_dir.clone(),
        source,
    })?;
    write_yaml_atomic(&folder_dir.join(FOLDER_MANIFEST_FILE), &folder.doc)?;
    for req in &folder.requests {
        req.validate()?;
        let req_path = unique_file_path(&folder_dir, &req.name, REQUEST_EXTENSION);
        write_yaml_atomic(&req_path, req)?;
    }
    for sub in &folder.folders {
        persist_imported_folder(&folder_dir, sub)?;
    }
    Ok(())
}

fn merge_imported_folder(
    parent_dir: &Path,
    existing_folders: &mut [FolderNode],
    folder: &testrix_core::ImportedFolder,
) -> Result<(), StorageError> {
    if let Some(existing_folder) = existing_folders
        .iter_mut()
        .find(|f| f.doc.name == folder.doc.name)
    {
        for req in &folder.requests {
            merge_or_create_request(
                &existing_folder.dir_path,
                &mut existing_folder.requests,
                req,
            )?;
        }
        for sub in &folder.folders {
            merge_imported_folder(
                &existing_folder.dir_path,
                &mut existing_folder.folders,
                sub,
            )?;
        }
    } else {
        persist_imported_folder(parent_dir, folder)?;
    }
    Ok(())
}

/// Merges incoming [`EnvironmentDoc`] profiles into `env_dir` as standalone `*.env.yaml` files
/// without deleting or overwriting any existing profiles or their variables.
pub fn merge_environments_into_dir(
    env_dir: &Path,
    incoming_envs: &[EnvironmentDoc],
) -> Result<(), StorageError> {
    if incoming_envs.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(env_dir).map_err(|source| StorageError::Io {
        path: env_dir.to_path_buf(),
        source,
    })?;

    let mut existing = if env_dir.is_dir() {
        load_environments(env_dir)?
    } else {
        Vec::new()
    };

    for incoming_env in incoming_envs {
        incoming_env.validate()?;
        if let Some(existing_env) = existing
            .iter_mut()
            .find(|e| e.doc.name.eq_ignore_ascii_case(&incoming_env.name))
        {
            let mut changed = false;
            for var in &incoming_env.variables {
                if !existing_env
                    .doc
                    .variables
                    .iter()
                    .any(|v| v.key.trim() == var.key.trim())
                {
                    existing_env.doc.variables.push(var.clone());
                    changed = true;
                }
            }
            if changed {
                save_environment(existing_env)?;
            }
        } else {
            let env_path = unique_file_path(env_dir, &incoming_env.name, ENV_EXTENSION);
            write_yaml_atomic(&env_path, incoming_env)?;
            existing.push(EnvironmentNode {
                file_path: env_path,
                doc: incoming_env.clone(),
            });
        }
    }

    Ok(())
}

/// Loads all shared environment profiles from `<workspace_root>/environments/*.env.yaml`.
pub fn load_workspace_environments(
    workspace_root: &Path,
) -> Result<Vec<EnvironmentNode>, StorageError> {
    let env_dir = workspace_root.join(ENVIRONMENTS_DIR);
    if !env_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut envs = load_environments(&env_dir)?;
    envs.sort_by_key(|a| a.doc.name.to_lowercase());
    Ok(envs)
}

/// Persists an [`ImportedProjectBundle`] (such as an OpenAPI 3.0 or Postman import) into `workspace_root`.
/// Imported environment profiles are merged into `<workspace_root>/environments/*.env.yaml`.
pub fn import_project_bundle(
    workspace_root: &Path,
    bundle: &ImportedProjectBundle,
) -> Result<ProjectNode, StorageError> {
    bundle.project.validate()?;
    let root_dir = unique_dir_path(workspace_root, &bundle.project.id);
    std::fs::create_dir_all(&root_dir).map_err(|source| StorageError::Io {
        path: root_dir.clone(),
        source,
    })?;

    write_yaml_atomic(&root_dir.join(PROJECT_MANIFEST_FILE), &bundle.project)?;

    let ws_env_dir = workspace_root.join(ENVIRONMENTS_DIR);
    merge_environments_into_dir(&ws_env_dir, &bundle.environments)?;

    for req in &bundle.root_requests {
        req.validate()?;
        let req_path = unique_file_path(&root_dir, &req.name, REQUEST_EXTENSION);
        write_yaml_atomic(&req_path, req)?;
    }

    for folder in &bundle.folders {
        persist_imported_folder(&root_dir, folder)?;
    }

    load_project(&root_dir)
}

/// Merges folders, requests, and missing environment variables from `bundle` into an existing
/// [`ProjectNode`] without deleting or overwriting any existing profiles or their variables.
pub fn import_bundle_into_project(
    project: &mut ProjectNode,
    bundle: &ImportedProjectBundle,
) -> Result<(), StorageError> {
    let root_dir = project.root_dir.clone();
    let env_dir = root_dir.join(ENVIRONMENTS_DIR);
    std::fs::create_dir_all(&env_dir).map_err(|source| StorageError::Io {
        path: env_dir.clone(),
        source,
    })?;

    // Merge environments: never delete or overwrite existing profiles or variables.
    for incoming_env in &bundle.environments {
        if let Some(existing_env) = project
            .environments
            .iter_mut()
            .find(|e| e.doc.name.eq_ignore_ascii_case(&incoming_env.name))
        {
            let mut changed = false;
            for var in &incoming_env.variables {
                if !existing_env
                    .doc
                    .variables
                    .iter()
                    .any(|v| v.key.trim() == var.key.trim())
                {
                    existing_env.doc.variables.push(var.clone());
                    changed = true;
                }
            }
            if changed {
                save_environment(existing_env)?;
            }
        } else if project.environments.is_empty() {
            incoming_env.validate()?;
            let env_path = unique_file_path(&env_dir, &incoming_env.name, ENV_EXTENSION);
            write_yaml_atomic(&env_path, incoming_env)?;
        }
    }

    // Merge root requests (match existing by method + url, else create new).
    for req in &bundle.root_requests {
        merge_or_create_request(&root_dir, &mut project.requests, req)?;
    }

    // Merge folders by name; create if missing, then merge their requests.
    for folder in &bundle.folders {
        merge_imported_folder(&root_dir, &mut project.folders, folder)?;
    }

    *project = load_project(&root_dir)?;
    Ok(())
}

fn merge_or_create_request(
    target_dir: &Path,
    existing_requests: &mut [RequestNode],
    incoming: &HttpRequestDoc,
) -> Result<(), StorageError> {
    if let Some(existing) = existing_requests.iter_mut().find(|r| {
        r.doc.request.method == incoming.request.method
            && r.doc.request.url == incoming.request.url
    }) {
        existing.doc.name = incoming.name.clone();
        if !incoming.description.is_empty() {
            existing.doc.description = incoming.description.clone();
        }
        existing.doc.request.params = incoming.request.params.clone();
        existing.doc.request.headers = incoming.request.headers.clone();
        existing.doc.request.body = incoming.request.body.clone();
        save_request(existing)?;
    } else {
        incoming.validate()?;
        let req_path = unique_file_path(target_dir, &incoming.name, REQUEST_EXTENSION);
        write_yaml_atomic(&req_path, incoming)?;
    }
    Ok(())
}

/// Saves a folder manifest (`_folder.yaml`).
pub fn save_folder(folder: &FolderNode) -> Result<(), StorageError> {
    folder.doc.validate()?;
    write_yaml_atomic(&folder.manifest_path, &folder.doc)
}

/// Creates a new folder directory and `_folder.yaml` under `parent_dir`.
pub fn create_folder(
    parent_dir: &Path,
    name: &str,
    order: i32,
) -> Result<FolderNode, StorageError> {
    let trimmed = name.trim();
    let folder_name = if trimmed.is_empty() {
        "New Folder"
    } else {
        trimmed
    };
    let dir_path = unique_dir_path(parent_dir, folder_name);
    std::fs::create_dir_all(&dir_path).map_err(|source| StorageError::Io {
        path: dir_path.clone(),
        source,
    })?;

    let mut doc = FolderDoc::new(generate_id("fld"), folder_name);
    doc.order = order;
    let manifest_path = dir_path.join(FOLDER_MANIFEST_FILE);
    write_yaml_atomic(&manifest_path, &doc)?;

    Ok(FolderNode {
        doc,
        dir_path,
        manifest_path,
        has_manifest: true,
        folders: Vec::new(),
        requests: Vec::new(),
    })
}

/// Saves an HTTP request YAML document (`*.request.yaml`).
pub fn save_request(request: &RequestNode) -> Result<(), StorageError> {
    request.doc.validate()?;
    write_yaml_atomic(&request.file_path, &request.doc)
}

/// Creates a new HTTP request YAML document under `parent_dir`.
pub fn create_request(
    parent_dir: &Path,
    name: &str,
    method: HttpMethod,
    url: &str,
) -> Result<RequestNode, StorageError> {
    let trimmed = name.trim();
    let req_name = if trimmed.is_empty() {
        "New Request"
    } else {
        trimmed
    };
    let file_path = unique_file_path(parent_dir, req_name, REQUEST_EXTENSION);
    let doc = HttpRequestDoc::new(generate_id("req"), req_name, method, url);
    write_yaml_atomic(&file_path, &doc)?;
    Ok(RequestNode { doc, file_path })
}

/// Duplicates an existing request into a sibling `*.request.yaml` with a new ID and name.
pub fn duplicate_request(source: &RequestNode) -> Result<RequestNode, StorageError> {
    let parent_dir = source
        .file_path
        .parent()
        .ok_or_else(|| StorageError::InvalidPath {
            path: source.file_path.clone(),
            reason: "request path has no parent directory".to_string(),
        })?;
    let copy_name = format!("{} Copy", source.doc.name);
    let file_path = unique_file_path(parent_dir, &copy_name, REQUEST_EXTENSION);
    let mut doc = source.doc.clone();
    doc.id = generate_id("req");
    doc.name = copy_name;
    write_yaml_atomic(&file_path, &doc)?;
    Ok(RequestNode { doc, file_path })
}

/// Moves an HTTP request YAML file to `target_dir`, removing the source file.
pub fn move_request(
    source_file: &Path,
    target_dir: &Path,
    doc: &HttpRequestDoc,
) -> Result<RequestNode, StorageError> {
    doc.validate()?;
    if source_file.parent() == Some(target_dir) {
        return Ok(RequestNode {
            doc: doc.clone(),
            file_path: source_file.to_path_buf(),
        });
    }
    let dest_path = unique_file_path(target_dir, &doc.name, REQUEST_EXTENSION);
    write_yaml_atomic(&dest_path, doc)?;
    if source_file.exists() && source_file != dest_path {
        std::fs::remove_file(source_file).map_err(|source| StorageError::Io {
            path: source_file.to_path_buf(),
            source,
        })?;
    }
    Ok(RequestNode {
        doc: doc.clone(),
        file_path: dest_path,
    })
}

/// Removes an HTTP request YAML file from disk.
pub fn delete_request(file_path: &Path) -> Result<(), StorageError> {
    if file_path.exists() {
        std::fs::remove_file(file_path).map_err(|source| StorageError::Io {
            path: file_path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

/// Removes a folder directory and all contained YAML files from disk.
pub fn delete_folder(dir_path: &Path) -> Result<(), StorageError> {
    if dir_path.exists() {
        std::fs::remove_dir_all(dir_path).map_err(|source| StorageError::Io {
            path: dir_path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

/// Removes a project directory and all its folders, requests, and environments from disk.
pub fn delete_project(root_dir: &Path) -> Result<(), StorageError> {
    delete_folder(root_dir)
}

/// Saves an environment profile YAML document (`*.env.yaml`).
pub fn save_environment(env: &EnvironmentNode) -> Result<(), StorageError> {
    env.doc.validate()?;
    let mut sorted = env.doc.clone();
    testrix_core::sort_env_variables(&mut sorted.variables);
    write_yaml_atomic(&env.file_path, &sorted)
}

/// Creates a new environment profile under `<project_root>/environments/`.
pub fn create_environment(
    project_root: &Path,
    name: &str,
) -> Result<EnvironmentNode, StorageError> {
    let trimmed = name.trim();
    let env_name = if trimmed.is_empty() {
        "New Profile"
    } else {
        trimmed
    };
    let env_dir = project_root.join(ENVIRONMENTS_DIR);
    std::fs::create_dir_all(&env_dir).map_err(|source| StorageError::Io {
        path: env_dir.clone(),
        source,
    })?;
    let file_path = unique_file_path(&env_dir, env_name, ENV_EXTENSION);
    let doc = EnvironmentDoc::new(generate_id("env"), env_name);
    write_yaml_atomic(&file_path, &doc)?;
    Ok(EnvironmentNode { doc, file_path })
}

/// Deletes an environment profile YAML file from disk.
pub fn delete_environment(file_path: &Path) -> Result<(), StorageError> {
    if file_path.exists() {
        std::fs::remove_file(file_path).map_err(|source| StorageError::Io {
            path: file_path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn read_yaml_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StorageError> {
    let content = std::fs::read_to_string(path).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    yaml_serde::from_str(&content).map_err(|source| {
        warn!(path = %path.display(), error = %source, "failed parsing YAML document");
        StorageError::YamlRead {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn is_hidden_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn sort_folders(folders: &mut [FolderNode]) {
    folders.sort_by(|a, b| {
        a.doc
            .order
            .cmp(&b.doc.order)
            .then_with(|| a.doc.name.to_lowercase().cmp(&b.doc.name.to_lowercase()))
    });
}

fn sort_requests(requests: &mut [RequestNode]) {
    requests.sort_by(|a, b| {
        a.doc
            .order
            .cmp(&b.doc.order)
            .then_with(|| a.doc.name.to_lowercase().cmp(&b.doc.name.to_lowercase()))
    });
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Returns the default workspace directory (`~/.testrix/workspace`, fallback `./.testrix-workspace`).
pub fn default_workspace_dir() -> PathBuf {
    if let Some(home) = home_dir() {
        home.join(".testrix").join("workspace")
    } else {
        PathBuf::from(".testrix-workspace")
    }
}

/// Returns the Testrix user configuration file path (`~/.testrix/config.yaml`).
pub fn config_file_path() -> PathBuf {
    if let Some(home) = home_dir() {
        home.join(".testrix").join("config.yaml")
    } else {
        PathBuf::from(".testrix").join("config.yaml")
    }
}

/// Persistent Testrix application configuration stored at `~/.testrix/config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestrixConfig {
    #[serde(default = "default_config_api_version")]
    pub api_version: String,
    pub workspace_dir: PathBuf,
    #[serde(default = "default_true")]
    pub remember_open_tabs: bool,
    #[serde(default = "default_true")]
    pub remember_responses: bool,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_share_server_url")]
    pub share_server_url: String,
    #[serde(default)]
    pub share_email: String,
    #[serde(default)]
    pub share_token: String,
    #[serde(default)]
    pub cloudflare_account_id: String,
    #[serde(default)]
    pub cloudflare_r2_bucket: String,
    #[serde(default)]
    pub cloudflare_api_token: String,
    #[serde(default)]
    pub cloudflare_pages_url: String,
    #[serde(default)]
    pub share_access_token: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_projects: Vec<PersistedShareRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_behavior: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme_mode: Option<String>,
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: String,
    #[serde(default = "default_editor_font_size")]
    pub editor_font_size: u16,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub key_bindings: std::collections::BTreeMap<String, String>,
}

fn default_ui_font_size() -> String {
    "small".to_string()
}

fn default_editor_font_size() -> u16 {
    13
}

/// Persisted remote share record stored in `~/.testrix/config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedShareRecord {
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub project_slug: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub share_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_info: String,
}

fn default_config_api_version() -> String {
    testrix_core::API_VERSION.to_string()
}

fn default_language() -> String {
    "en".to_string()
}

fn default_share_server_url() -> String {
    "https://testrix.bitifyware.com".to_string()
}

const fn default_true() -> bool {
    true
}

impl Default for TestrixConfig {
    fn default() -> Self {
        Self {
            api_version: default_config_api_version(),
            workspace_dir: default_workspace_dir(),
            remember_open_tabs: true,
            remember_responses: true,
            language: default_language(),
            share_server_url: default_share_server_url(),
            share_email: String::new(),
            share_token: String::new(),
            cloudflare_account_id: String::new(),
            cloudflare_r2_bucket: String::new(),
            cloudflare_api_token: String::new(),
            cloudflare_pages_url: String::new(),
            share_access_token: String::new(),
            shared_projects: Vec::new(),
            close_behavior: None,
            theme_mode: None,
            ui_font_size: default_ui_font_size(),
            editor_font_size: default_editor_font_size(),
            key_bindings: std::collections::BTreeMap::new(),
        }
    }
}

/// Loads `~/.testrix/config.yaml`, initializing it with defaults on first startup if missing.
pub fn init_or_load_config() -> Result<TestrixConfig, StorageError> {
    init_or_load_config_at(&config_file_path())
}

/// Loads a Testrix config file at `config_path`, creating it if missing.
pub fn init_or_load_config_at(config_path: &Path) -> Result<TestrixConfig, StorageError> {
    if config_path.is_file() {
        let mut cfg: TestrixConfig = read_yaml_file(config_path)?;
        if cfg.workspace_dir.as_os_str().is_empty() {
            cfg.workspace_dir = default_workspace_dir();
        }
        return Ok(cfg);
    }

    let cfg = TestrixConfig::default();
    write_yaml_atomic(config_path, &cfg)?;
    Ok(cfg)
}

/// Persists `workspace_dir` into `~/.testrix/config.yaml`.
pub fn save_config_workspace_dir(workspace_dir: &Path) -> Result<(), StorageError> {
    save_config_workspace_dir_at(&config_file_path(), workspace_dir)
}

/// Persists `workspace_dir` into `config_path`.
pub fn save_config_workspace_dir_at(
    config_path: &Path,
    workspace_dir: &Path,
) -> Result<(), StorageError> {
    let mut cfg = if config_path.is_file() {
        read_yaml_file::<TestrixConfig>(config_path).unwrap_or_default()
    } else {
        TestrixConfig::default()
    };
    cfg.workspace_dir = workspace_dir.to_path_buf();
    write_yaml_atomic(config_path, &cfg)
}

/// Persists preference toggles, language, and close behavior into `~/.testrix/config.yaml`.
pub fn save_config_preferences(
    remember_open_tabs: bool,
    remember_responses: bool,
    language: &str,
    close_behavior: Option<&str>,
) -> Result<(), StorageError> {
    save_config_preferences_at(
        &config_file_path(),
        remember_open_tabs,
        remember_responses,
        language,
        close_behavior,
    )
}

/// Persists preference toggles, language, and close behavior into `config_path`.
pub fn save_config_preferences_at(
    config_path: &Path,
    remember_open_tabs: bool,
    remember_responses: bool,
    language: &str,
    close_behavior: Option<&str>,
) -> Result<(), StorageError> {
    let mut cfg = if config_path.is_file() {
        read_yaml_file::<TestrixConfig>(config_path).unwrap_or_default()
    } else {
        TestrixConfig::default()
    };
    cfg.remember_open_tabs = remember_open_tabs;
    cfg.remember_responses = remember_responses;
    if !language.trim().is_empty() {
        cfg.language = language.trim().to_string();
    }
    cfg.close_behavior = close_behavior
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    write_yaml_atomic(config_path, &cfg)
}

/// Persists theme mode, UI font size, and editor font size into `~/.testrix/config.yaml`.
pub fn save_config_appearance(
    theme_mode: Option<&str>,
    ui_font_size: &str,
    editor_font_size: u16,
) -> Result<(), StorageError> {
    save_config_appearance_at(&config_file_path(), theme_mode, ui_font_size, editor_font_size)
}

/// Persists theme mode, UI font size, and editor font size into `config_path`.
pub fn save_config_appearance_at(
    config_path: &Path,
    theme_mode: Option<&str>,
    ui_font_size: &str,
    editor_font_size: u16,
) -> Result<(), StorageError> {
    let mut cfg = if config_path.is_file() {
        read_yaml_file::<TestrixConfig>(config_path).unwrap_or_default()
    } else {
        TestrixConfig::default()
    };
    cfg.theme_mode = theme_mode
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if !ui_font_size.trim().is_empty() {
        cfg.ui_font_size = ui_font_size.trim().to_string();
    }
    cfg.editor_font_size = editor_font_size.clamp(10, 24);
    write_yaml_atomic(config_path, &cfg)
}

/// Persists custom keybindings into `~/.testrix/config.yaml`.
pub fn save_config_key_bindings(
    key_bindings: &std::collections::BTreeMap<String, String>,
) -> Result<(), StorageError> {
    save_config_key_bindings_at(&config_file_path(), key_bindings)
}

/// Persists custom keybindings into `config_path`.
pub fn save_config_key_bindings_at(
    config_path: &Path,
    key_bindings: &std::collections::BTreeMap<String, String>,
) -> Result<(), StorageError> {
    let mut cfg = if config_path.is_file() {
        read_yaml_file::<TestrixConfig>(config_path).unwrap_or_default()
    } else {
        TestrixConfig::default()
    };
    cfg.key_bindings = key_bindings.clone();
    write_yaml_atomic(config_path, &cfg)
}

/// Share server login & Cloudflare R2 credentials persisted in `~/.testrix/config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShareSettings {
    pub share_server_url: String,
    pub share_email: String,
    pub share_token: String,
    pub cloudflare_account_id: String,
    pub cloudflare_r2_bucket: String,
    pub cloudflare_api_token: String,
    pub cloudflare_pages_url: String,
    pub share_access_token: String,
}

impl From<&TestrixConfig> for ShareSettings {
    fn from(cfg: &TestrixConfig) -> Self {
        Self {
            share_server_url: cfg.share_server_url.clone(),
            share_email: cfg.share_email.clone(),
            share_token: cfg.share_token.clone(),
            cloudflare_account_id: cfg.cloudflare_account_id.clone(),
            cloudflare_r2_bucket: cfg.cloudflare_r2_bucket.clone(),
            cloudflare_api_token: cfg.cloudflare_api_token.clone(),
            cloudflare_pages_url: cfg.cloudflare_pages_url.clone(),
            share_access_token: cfg.share_access_token.clone(),
        }
    }
}

/// Persists share server & Cloudflare R2 settings into `~/.testrix/config.yaml`.
pub fn save_config_share_settings(settings: &ShareSettings) -> Result<(), StorageError> {
    save_config_share_settings_at(&config_file_path(), settings)
}

/// Persists share server & Cloudflare R2 settings into `config_path`.
pub fn save_config_share_settings_at(
    config_path: &Path,
    settings: &ShareSettings,
) -> Result<(), StorageError> {
    let mut cfg = if config_path.is_file() {
        read_yaml_file::<TestrixConfig>(config_path).unwrap_or_default()
    } else {
        TestrixConfig::default()
    };
    if !settings.share_server_url.trim().is_empty() {
        cfg.share_server_url = settings.share_server_url.trim().to_string();
    }
    cfg.share_email = settings.share_email.trim().to_string();
    cfg.share_token = settings.share_token.trim().to_string();
    cfg.cloudflare_account_id = settings.cloudflare_account_id.trim().to_string();
    cfg.cloudflare_r2_bucket = settings.cloudflare_r2_bucket.trim().to_string();
    cfg.cloudflare_api_token = settings.cloudflare_api_token.trim().to_string();
    cfg.cloudflare_pages_url = settings.cloudflare_pages_url.trim().to_string();
    cfg.share_access_token = settings.share_access_token.trim().to_string();
    write_yaml_atomic(config_path, &cfg)
}

/// Persists active remote share records in `~/.testrix/config.yaml`.
pub fn save_config_shared_projects(shares: &[PersistedShareRecord]) -> Result<(), StorageError> {
    save_config_shared_projects_at(&config_file_path(), shares)
}

/// Persists active remote share records at `config_path`.
pub fn save_config_shared_projects_at(
    config_path: &Path,
    shares: &[PersistedShareRecord],
) -> Result<(), StorageError> {
    let mut cfg = init_or_load_config_at(config_path)?;
    cfg.shared_projects = shares.to_vec();
    write_yaml_atomic(config_path, &cfg)
}
