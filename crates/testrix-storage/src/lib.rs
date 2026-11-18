//! YAML document persistence and workspace management for Testrix (`testrix.io/v1`).

mod error;
mod seed;
mod slug;
mod workspace;

pub use error::StorageError;
pub use seed::seed_starter_workspace;
pub use slug::slugify;
pub use workspace::{
    ShareSettings, TestrixConfig, WorkspaceTabState, clear_saved_responses, config_file_path,
    create_environment, create_folder, create_project, create_request, default_workspace_dir,
    delete_environment, delete_folder, delete_project, delete_request, duplicate_request,
    ensure_seeded_workspace, import_bundle_into_project, import_project_bundle,
    init_or_load_config, init_or_load_config_at, load_project, load_request_response,
    load_workspace, load_workspace_tabs, save_config_preferences, save_config_preferences_at,
    save_config_share_settings, save_config_share_settings_at, save_config_workspace_dir,
    save_config_workspace_dir_at, save_environment, save_folder, save_project, save_request,
    save_request_response, save_workspace_state, save_workspace_tabs, to_yaml_pretty,
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
        assert_eq!(project.environments.len(), 3);
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

        save_config_preferences_at(&cfg_path, false, false, "zh-CN").unwrap();
        let prefs_reloaded = init_or_load_config_at(&cfg_path).unwrap();
        assert!(!prefs_reloaded.remember_open_tabs);
        assert!(!prefs_reloaded.remember_responses);
        assert_eq!(prefs_reloaded.language, "zh-CN");
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
        assert_eq!(proj.environments.len(), 1);

        let ws = load_workspace(dir.path()).unwrap();
        assert_eq!(ws.projects.len(), 1);
        assert_eq!(ws.projects[0].doc.name, "AppCore");

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
}
