//! YAML document persistence and workspace management for Testrix (`testrix.io/v1`).

mod error;
mod git;
mod seed;
mod slug;
mod workspace;

pub use error::StorageError;
pub use git::{
    GitCommitInfo, SyncBaseSnapshot, ThreeWayMergeResult, apply_synced_files_and_commit,
    collect_workspace_tracked_files, commit_workspace_change, ensure_workspace_git_repo,
    head_commit_hash, is_git_repo, list_file_commits, load_sync_base, merge_file_three_way,
    read_file_at_commit, restore_file_from_commit, save_sync_base, three_way_merge_trees,
    workspace_relative_path,
};
pub use seed::seed_starter_workspace;
pub use slug::slugify;
pub use workspace::{
    PersistedShareRecord, ShareSettings, SyncSettings, TestrixConfig, WorkspaceTabState,
    clear_saved_responses, config_file_path, create_environment, create_folder, create_project,
    create_request, default_workspace_dir, delete_environment, delete_folder, delete_project,
    delete_request, duplicate_request, ensure_seeded_workspace, import_bundle_into_project,
    import_project_bundle, init_or_load_config, init_or_load_config_at, load_global_environment,
    load_project,
    load_request_response, load_workspace, load_workspace_environments,
    load_workspace_explorer_state, load_workspace_history, load_workspace_sets,
    load_workspace_tabs, merge_environments_into_dir, move_request, save_config_appearance,
    save_config_appearance_at, save_config_key_bindings, save_config_key_bindings_at,
    save_config_preferences, save_config_preferences_at, save_config_share_settings,
    save_config_share_settings_at, save_config_shared_projects, save_config_shared_projects_at,
    save_config_sync_settings, save_config_sync_settings_at, save_config_workspace_dir,
    save_config_workspace_dir_at, save_environment, save_folder, save_global_environment,
    save_project, save_request,
    save_request_response, save_workspace_explorer_state, save_workspace_history,
    save_workspace_sets, save_workspace_state, save_workspace_tabs, to_yaml_pretty,
    validate_yaml_file, write_yaml_atomic,
};

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use testrix_core::{AuthConfig, EffectiveAuth, HttpMethod, resolve_request_auth};

    use super::*;

    #[test]
    fn seeds_and_reloads_starter_workspace_yaml_documents() {
        let dir = TempDir::new().unwrap();
        let tree = ensure_seeded_workspace(dir.path()).unwrap();

        assert_eq!(tree.projects.len(), 1);
        let project = &tree.projects[0];
        assert_eq!(project.doc.name, "Starter Platform API");
        assert_eq!(tree.environments.len(), 3);
        assert!(project.request_count() >= 8);

        // Verify Project -> Folder -> Nested Folder -> Request auth inheritance after YAML round-trip.
        let nested_auth = resolve_request_auth(project, "req-security-rotate").unwrap();
        assert!(matches!(
            nested_auth.effective,
            EffectiveAuth::ApiKey { .. }
        ));

        let root_auth = resolve_request_auth(project, "req-inspect-get").unwrap();
        assert!(matches!(root_auth.effective, EffectiveAuth::Bearer { .. }));

        let none_auth = resolve_request_auth(project, "req-security-public-ping").unwrap();
        assert_eq!(none_auth.effective, EffectiveAuth::None);
    }

    #[test]
    fn creates_updates_and_persists_request_and_folder_yaml_files() {
        let dir = TempDir::new().unwrap();
        let proj = create_project(dir.path(), "Orders API", "https://orders.test").unwrap();
        assert_eq!(
            proj.root_dir.file_name().and_then(|n| n.to_str()),
            Some(proj.doc.id.as_str())
        );

        let mut folder = create_folder(&proj.root_dir, "V1 Orders", 5).unwrap();
        folder.doc.auth = AuthConfig::Basic {
            username: "svc".to_string(),
            password: "secret".to_string(),
        };
        save_folder(&folder).unwrap();

        let mut req =
            create_request(&folder.dir_path, "List Orders", HttpMethod::Get, "/orders").unwrap();
        assert!(req.file_path.extension().is_some());
        assert!(req.file_path.to_string_lossy().ends_with(".request.yaml"));

        req.doc.request.method = HttpMethod::Post;
        save_request(&req).unwrap();

        let reloaded = load_workspace(dir.path()).unwrap();
        let reloaded_proj = &reloaded.projects[0];
        let auth = resolve_request_auth(reloaded_proj, &req.doc.id).unwrap();
        assert!(matches!(auth.effective, EffectiveAuth::Basic { .. }));
        let (_, found_req) = reloaded_proj.find_request(&req.doc.id).unwrap();
        assert_eq!(found_req.doc.request.method, HttpMethod::Post);
    }

    #[test]
    fn initializes_loads_and_updates_testrix_config_yaml() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.yaml");

        let created = init_or_load_config_at(&cfg_path).unwrap();
        assert!(cfg_path.is_file());
        assert_eq!(created.api_version, testrix_core::API_VERSION);

        let custom_ws = dir.path().join("custom-ws");
        save_config_workspace_dir_at(&cfg_path, &custom_ws).unwrap();

        let reloaded = init_or_load_config_at(&cfg_path).unwrap();
        assert_eq!(reloaded.workspace_dir, custom_ws);
        assert!(reloaded.remember_open_tabs);
        assert!(reloaded.remember_responses);
        assert_eq!(reloaded.close_behavior, None);

        save_config_preferences_at(&cfg_path, false, false, "zh-CN", Some("tray")).unwrap();
        let prefs_reloaded = init_or_load_config_at(&cfg_path).unwrap();
        assert!(!prefs_reloaded.remember_open_tabs);
        assert!(!prefs_reloaded.remember_responses);
        assert_eq!(prefs_reloaded.language, "zh-CN");
        assert_eq!(prefs_reloaded.close_behavior.as_deref(), Some("tray"));
    }

    #[test]
    fn persists_and_restores_open_tabs_and_request_responses() {
        let dir = TempDir::new().unwrap();
        save_workspace_tabs(
            dir.path(),
            &["req:r1".to_string(), "req:r2".to_string()],
            Some("req:r2"),
        )
        .unwrap();
        let tabs = load_workspace_tabs(dir.path());
        assert_eq!(tabs.open_tabs, vec!["req:r1", "req:r2"]);
        assert_eq!(tabs.active_tab_key.as_deref(), Some("req:r2"));

        let snap = testrix_core::HttpResponseSnapshot {
            status_code: 200,
            reason: "OK".to_string(),
            http_version: "HTTP/1.1".to_string(),
            duration_ms: 12,
            size_bytes: 11,
            headers: Vec::new(),
            body_bytes: Vec::new(),
            body_text: r#"{"ok":true}"#.to_string(),
            pretty_body: r#"{"ok":true}"#.to_string(),
            content_type: Some("application/json".to_string()),
            sent_request: testrix_core::SentRequestSnapshot {
                method: HttpMethod::Get,
                url: "https://example.com".to_string(),
                headers: Vec::new(),
                body_preview: String::new(),
            },
        };
        save_request_response(dir.path(), "r1", &snap).unwrap();
        let loaded_snap = load_request_response(dir.path(), "r1").unwrap();
        assert_eq!(loaded_snap.status_code, 200);
        assert_eq!(loaded_snap.body_text, r#"{"ok":true}"#);

        clear_saved_responses(dir.path()).unwrap();
        assert!(load_request_response(dir.path(), "r1").is_none());
    }

    #[test]
    fn imports_openapi_3_0_bundle_and_reloads_from_disk() {
        let dir = TempDir::new().unwrap();
        let openapi_json = r#"{
          "openapi": "3.0.1",
          "info": { "title": "AppCore", "version": "1.0.0" },
          "tags": [{ "name": "门诊开单" }],
          "servers": [{ "url": "http://localhost:30001/app-core", "description": "本地231" }],
          "paths": {
            "/outpPresc/operate/saveOutpPresc": {
              "post": {
                "summary": "处方保存",
                "tags": ["门诊开单"],
                "requestBody": {
                  "content": {
                    "application/json": { "example": { "packFlag": "1" } }
                  }
                }
              }
            }
          }
        }"#;
        let bundle = testrix_core::parse_openapi_3_0(openapi_json).unwrap();
        let proj = import_project_bundle(dir.path(), &bundle).unwrap();
        assert_eq!(proj.doc.name, "AppCore");
        assert_eq!(proj.folders.len(), 1);
        assert_eq!(proj.folders[0].requests.len(), 1);

        let ws = load_workspace(dir.path()).unwrap();
        assert_eq!(ws.projects.len(), 1);
        assert_eq!(ws.projects[0].doc.name, "AppCore");
        assert_eq!(ws.environments.len(), 1);

        // Importing another OpenAPI bundle into the existing project preserves its profiles & variables.
        let mut target_proj = ws.projects[0].clone();
        let mut custom_env = testrix_core::EnvironmentDoc::new("env-custom", "Custom");
        custom_env
            .variables
            .push(testrix_core::EnvVariable::new("api-token", "keep-me"));
        let env_path = target_proj.root_dir.join("environments/custom.env.yaml");
        write_yaml_atomic(&env_path, &custom_env).unwrap();
        target_proj = load_project(&target_proj.root_dir).unwrap();

        import_bundle_into_project(&mut target_proj, &bundle).unwrap();
        let custom_reloaded = target_proj
            .environments
            .iter()
            .find(|e| e.doc.id == "env-custom")
            .expect("custom profile should be preserved");
        assert_eq!(custom_reloaded.doc.variables[0].value, "keep-me");
        // Merging the same folder name reuses the existing folder instead of duplicating it.
        assert_eq!(target_proj.folders.len(), 1);
        assert_eq!(target_proj.folders[0].requests.len(), 1);
    }

    #[test]
    fn persists_share_and_cloudflare_settings_in_config_yaml() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.yaml");
        let _ = init_or_load_config_at(&cfg_path).unwrap();

        let settings = ShareSettings {
            share_server_url: "https://share.example.com".to_string(),
            share_email: "dev@example.com".to_string(),
            share_token: "tok-abc".to_string(),
            cloudflare_account_id: "cf-acc-1".to_string(),
            cloudflare_r2_bucket: "my-bucket".to_string(),
            cloudflare_api_token: "cf-tok-xyz".to_string(),
            cloudflare_pages_url: "https://docs.pages.dev".to_string(),
            share_access_token: "viewer-secret-123".to_string(),
        };
        save_config_share_settings_at(&cfg_path, &settings).unwrap();

        let reloaded = init_or_load_config_at(&cfg_path).unwrap();
        assert_eq!(ShareSettings::from(&reloaded), settings);
    }

    #[test]
    fn moves_request_across_folders_and_reloads_from_disk() {
        let dir = TempDir::new().unwrap();
        let proj = create_project(dir.path(), "Demo Project", "https://api.example.com").unwrap();
        let folder_a = create_folder(&proj.root_dir, "Folder A", 0).unwrap();
        let folder_b = create_folder(&proj.root_dir, "Folder B", 1).unwrap();

        let req =
            create_request(&folder_a.dir_path, "Login", HttpMethod::Post, "/login").unwrap();
        assert!(req.file_path.exists());

        let moved = move_request(&req.file_path, &folder_b.dir_path, &req.doc).unwrap();
        assert!(!req.file_path.exists());
        assert!(moved.file_path.exists());
        assert_eq!(moved.file_path.parent(), Some(folder_b.dir_path.as_path()));

        let reloaded = load_project(&proj.root_dir).unwrap();
        let reloaded_a = reloaded
            .folders
            .iter()
            .find(|f| f.doc.id == folder_a.doc.id)
            .unwrap();
        let reloaded_b = reloaded
            .folders
            .iter()
            .find(|f| f.doc.id == folder_b.doc.id)
            .unwrap();
        assert!(reloaded_a.requests.is_empty());
        assert_eq!(reloaded_b.requests.len(), 1);
        assert_eq!(reloaded_b.requests[0].doc.id, req.doc.id);
    }

    #[test]
    fn persists_workspace_history_and_sets() {
        use testrix_core::{
            HttpRequestDoc, RequestHistoryEntry, RequestSet, RequestSetFolder, RequestSetItem,
        };

        let dir = TempDir::new().unwrap();
        let history = vec![RequestHistoryEntry {
            id: "hist-1".to_string(),
            executed_at: "2026-11-19T17:00:00+08:00".to_string(),
            project_id: Some("proj-1".to_string()),
            request_id: Some("req-1".to_string()),
            env_id: None,
            doc: HttpRequestDoc::new("req-1", "Get Users", HttpMethod::Get, "/users"),
            status_code: Some(200),
            duration_ms: Some(42),
        }];
        save_workspace_history(dir.path(), &history).unwrap();
        let loaded_history = load_workspace_history(dir.path());
        assert_eq!(loaded_history, history);

        let sets = vec![RequestSet {
            id: "set-1".to_string(),
            name: "Smoke Suite".to_string(),
            folders: vec![RequestSetFolder {
                id: "sfolder-1".to_string(),
                name: "Auth".to_string(),
                folders: Vec::new(),
                requests: vec![RequestSetItem {
                    id: "sitem-1".to_string(),
                    project_id: "proj-1".to_string(),
                    request_id: "req-1".to_string(),
                    name: "Get Users".to_string(),
                    method: HttpMethod::Get,
                    url: "/users".to_string(),
                }],
            }],
            requests: Vec::new(),
        }];
        save_workspace_sets(dir.path(), &sets).unwrap();
        let loaded_sets = load_workspace_sets(dir.path());
        assert_eq!(loaded_sets, sets);
    }

    #[test]
    fn initializes_and_uses_existing_git_repo_and_commits_request_history() {
        let dir = TempDir::new().unwrap();
        let tree = ensure_seeded_workspace(dir.path()).unwrap();

        assert!(ensure_workspace_git_repo(dir.path()).unwrap());
        assert!(!ensure_workspace_git_repo(dir.path()).unwrap());
        assert!(is_git_repo(dir.path()));

        let proj_dir = &tree.projects[0].root_dir;
        let mut req = create_request(proj_dir, "List Items", HttpMethod::Get, "/items").unwrap();
        let c1 = commit_workspace_change(dir.path(), "Create request: List Items").unwrap();
        assert!(c1.is_some());

        req.doc.request.method = HttpMethod::Post;
        req.doc.request.url = "/items/v2".to_string();
        save_request(&req).unwrap();
        let c2 = commit_workspace_change(dir.path(), "Update request: List Items").unwrap();
        assert!(c2.is_some());

        // No-op commit when clean returns None.
        assert!(
            commit_workspace_change(dir.path(), "Noop")
                .unwrap()
                .is_none()
        );

        let commits = list_file_commits(dir.path(), &req.file_path, 20).unwrap();
        assert!(commits.len() >= 2);
        assert_eq!(commits[0].message, "Update request: List Items");
        assert_eq!(commits[1].message, "Create request: List Items");

        // Restore back to the first commit.
        let restored = restore_file_from_commit(
            dir.path(),
            &req.file_path,
            &commits[1].hash,
            commits[1].file_path_at_commit.as_deref(),
        )
        .unwrap();
        assert!(restored.contains("url: /items"));
        assert!(!restored.contains("url: /items/v2"));

        let commits_after = list_file_commits(dir.path(), &req.file_path, 20).unwrap();
        assert_eq!(commits_after.len(), commits.len() + 1);
        assert!(commits_after[0].message.starts_with("Restore "));
    }

    #[test]
    fn three_way_merge_preserves_both_sides_and_detects_conflicts() {
        use std::collections::BTreeMap;

        let mut base = BTreeMap::new();
        base.insert("a.request.yaml".to_string(), "name: A\nurl: /a\n".to_string());
        base.insert("b.request.yaml".to_string(), "name: B\nurl: /b\n".to_string());
        base.insert(
            "d.request.yaml".to_string(),
            "line1\nline2\nline3\nline4\nline5\n".to_string(),
        );

        let mut local = base.clone();
        local.insert(
            "a.request.yaml".to_string(),
            "name: A\nurl: /a-local\n".to_string(),
        );
        local.insert("c.request.yaml".to_string(), "name: C\nurl: /c\n".to_string());
        local.insert(
            "d.request.yaml".to_string(),
            "line1-local\nline2\nline3\nline4\nline5\n".to_string(),
        );

        let mut remote = base.clone();
        remote.insert(
            "b.request.yaml".to_string(),
            "name: B\nurl: /b-remote\n".to_string(),
        );
        remote.insert(
            "d.request.yaml".to_string(),
            "line1\nline2\nline3\nline4\nline5-remote\n".to_string(),
        );

        let merged = three_way_merge_trees(&base, &local, &remote).unwrap();
        assert!(merged.conflicts.is_empty());
        assert!(merged.local_changed);
        assert!(merged.remote_changed);
        assert_eq!(
            merged.merged_files.get("a.request.yaml").unwrap(),
            "name: A\nurl: /a-local\n"
        );
        assert_eq!(
            merged.merged_files.get("b.request.yaml").unwrap(),
            "name: B\nurl: /b-remote\n"
        );
        assert_eq!(
            merged.merged_files.get("c.request.yaml").unwrap(),
            "name: C\nurl: /c\n"
        );
        assert_eq!(
            merged.merged_files.get("d.request.yaml").unwrap(),
            "line1-local\nline2\nline3\nline4\nline5-remote\n"
        );

        // Conflicting edit on same line in a YAML file: must keep valid YAML (no `<<<<<<<` markers)
        let mut local_c = base.clone();
        local_c.insert(
            "a.request.yaml".to_string(),
            "name: A\nurl: /local-conflict\n".to_string(),
        );
        let mut remote_c = base.clone();
        remote_c.insert(
            "a.request.yaml".to_string(),
            "name: A\nurl: /remote-conflict\n".to_string(),
        );
        let conflict_res = three_way_merge_trees(&base, &local_c, &remote_c).unwrap();
        assert_eq!(conflict_res.conflicts, vec!["a.request.yaml".to_string()]);
        assert_eq!(
            conflict_res.merged_files.get("a.request.yaml").unwrap(),
            "name: A\nurl: /local-conflict\n"
        );
        assert!(
            !conflict_res
                .merged_files
                .get("a.request.yaml")
                .unwrap()
                .contains("<<<<<<<")
        );

        // Conflicting edit on a plain text file keeps standard conflict markers
        let mut base_txt = BTreeMap::new();
        base_txt.insert("notes.txt".to_string(), "hello\n".to_string());
        let mut local_txt = BTreeMap::new();
        local_txt.insert("notes.txt".to_string(), "hello local\n".to_string());
        let mut remote_txt = BTreeMap::new();
        remote_txt.insert("notes.txt".to_string(), "hello remote\n".to_string());
        let txt_res = three_way_merge_trees(&base_txt, &local_txt, &remote_txt).unwrap();
        assert_eq!(txt_res.conflicts, vec!["notes.txt".to_string()]);
        assert!(txt_res.merged_files.get("notes.txt").unwrap().contains("<<<<<<<"));
    }

    #[test]
    fn merges_environment_yaml_variables_from_fresh_workspace_and_remote() {
        use std::collections::BTreeMap;
        use testrix_core::{EnvVariable, EnvironmentDoc};

        let mut local_env = EnvironmentDoc::new("env-local-1", "Development");
        local_env
            .variables
            .push(EnvVariable::new("LOCAL_ONLY", "local-val"));

        let mut remote_env = EnvironmentDoc::new("env-remote-1", "Development");
        remote_env
            .variables
            .push(EnvVariable::new("API_KEY", "secret-123"));
        remote_env
            .variables
            .push(EnvVariable::new("BASE_URL", "https://api.example.com"));

        let base = BTreeMap::new();
        let mut local = BTreeMap::new();
        local.insert(
            "environments/development.env.yaml".to_string(),
            to_yaml_pretty(&local_env).unwrap(),
        );
        let mut remote = BTreeMap::new();
        remote.insert(
            "environments/development.env.yaml".to_string(),
            to_yaml_pretty(&remote_env).unwrap(),
        );

        let merged = three_way_merge_trees(&base, &local, &remote).unwrap();
        assert!(merged.conflicts.is_empty());
        assert!(merged.local_changed);
        assert!(merged.remote_changed);

        let merged_yaml = merged
            .merged_files
            .get("environments/development.env.yaml")
            .unwrap();
        let merged_doc: EnvironmentDoc = yaml_serde::from_str(merged_yaml).unwrap();
        assert!(merged_doc.variables.iter().any(|v| v.key == "API_KEY" && v.value == "secret-123"));
        assert!(merged_doc.variables.iter().any(|v| v.key == "BASE_URL" && v.value == "https://api.example.com"));
        assert!(merged_doc.variables.iter().any(|v| v.key == "LOCAL_ONLY" && v.value == "local-val"));
    }
}
