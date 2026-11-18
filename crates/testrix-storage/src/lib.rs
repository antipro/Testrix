//! YAML document persistence and workspace management for Testrix (`testrix.io/v1`).

mod error;
mod seed;
mod slug;
mod workspace;

pub use error::StorageError;
pub use seed::seed_starter_workspace;
pub use slug::slugify;
pub use workspace::{
    TestrixConfig, config_file_path, create_environment, create_folder, create_project,
    create_request, default_workspace_dir, delete_environment, delete_folder, delete_request,
    duplicate_request, ensure_seeded_workspace, init_or_load_config, init_or_load_config_at,
    load_project, load_workspace, save_config_workspace_dir, save_config_workspace_dir_at,
    save_environment, save_folder, save_project, save_request, save_workspace_state,
    to_yaml_pretty, write_yaml_atomic,
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
    }
}
