//! Core domain models, auth inheritance resolution, environment interpolation,
//! request materialization, and explorer filtering for Testrix.

mod apifox;
mod auth;
mod codegen;
mod error;
mod materialize;
mod model;
mod openapi;
mod postman;
mod response;
mod response_meta;
mod search;
mod template;

pub use apifox::parse_apifox;
pub use auth::{
    AuthChainStep, AuthNodeRef, ResolvedAuth, resolve_folder_auth, resolve_folder_chain_auth,
    resolve_project_auth, resolve_request_auth, resolve_request_doc_auth,
};
pub use codegen::{CodegenTarget, generate_request_code, parse_request_script};
pub use error::CoreError;
pub use materialize::{
    PreparedBody, PreparedHeader, PreparedMultipartPart, PreparedRequest,
    extract_url_query_params, materialize_request, materialize_request_doc,
};
pub use model::{
    API_VERSION, ApiKeyLocation, AuthConfig, DocumentKind, EffectiveAuth, EnvVariable,
    EnvironmentDoc, EnvironmentNode, ExplorerNode, FolderDoc, FolderNode, HttpMethod,
    HttpRequestDoc, HttpRequestSpec, KeyValueRow, MultipartField, MultipartFieldType, ProjectDoc,
    ProjectNode, RawBodyFormat, RequestBody, RequestNode, RequestSettings, WorkspaceTree,
    generate_id,
};
pub use openapi::{
    ImportedFolder, ImportedProjectBundle, export_openapi_3_0, merge_environments_into_bundle,
    parse_openapi_3_0,
};
pub use postman::{
    parse_postman_collection, parse_postman_environment, parse_postman_zip,
};
pub use response::{
    HttpResponseSnapshot, RequestExecutionState, ResponseHeader, SentRequestSnapshot,
    detect_body_language, format_pretty_body,
};
pub use response_meta::{
    ResponseExample, ResponseMetaField, extract_and_merge_response_meta, extract_response_meta,
    merge_response_meta,
};
pub use search::{FilteredFolder, FilteredProject, filter_workspace};
pub use template::{
    TemplateResult, env_variable_map, interpolate_template, interpolate_with_map,
    rename_template_variables,
};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn sample_project() -> ProjectNode {
        let mut project_doc = ProjectDoc::new("proj-1", "Sample API");
        project_doc.base_url = "{{base_url}}/api".to_string();
        project_doc.auth = AuthConfig::Bearer {
            token: "{{project_token}}".to_string(),
            prefix: "Bearer".to_string(),
        };
        project_doc.default_headers = vec![KeyValueRow::new("Accept", "application/json")];

        let mut req_inherited = HttpRequestDoc::new(
            "req-inherited",
            "Get Profile",
            HttpMethod::Get,
            "/v1/profile",
        );
        req_inherited.request.params = vec![KeyValueRow::new("expand", "{{expand_field}}")];

        let mut req_override = HttpRequestDoc::new(
            "req-override",
            "Create Item",
            HttpMethod::Post,
            "https://override.example.com/items",
        );
        req_override.request.auth = AuthConfig::None;
        req_override.request.body = RequestBody::Json {
            content: r#"{"owner":"{{user}}"}"#.to_string(),
        };

        let mut leaf_folder_doc = FolderDoc::new("fld-leaf", "Leaf");
        leaf_folder_doc.auth = AuthConfig::Inherit;

        let mut parent_folder_doc = FolderDoc::new("fld-parent", "Admin");
        parent_folder_doc.auth = AuthConfig::ApiKey {
            key: "X-Admin-Key".to_string(),
            value: "{{admin_key}}".to_string(),
            location: ApiKeyLocation::Header,
        };

        let leaf_folder = FolderNode {
            doc: leaf_folder_doc,
            dir_path: PathBuf::from("admin/leaf"),
            manifest_path: PathBuf::from("admin/leaf/_folder.yaml"),
            has_manifest: true,
            folders: vec![],
            requests: vec![RequestNode {
                doc: req_inherited,
                file_path: PathBuf::from("admin/leaf/get-profile.request.yaml"),
            }],
        };

        let parent_folder = FolderNode {
            doc: parent_folder_doc,
            dir_path: PathBuf::from("admin"),
            manifest_path: PathBuf::from("admin/_folder.yaml"),
            has_manifest: true,
            folders: vec![leaf_folder],
            requests: vec![RequestNode {
                doc: req_override,
                file_path: PathBuf::from("admin/create-item.request.yaml"),
            }],
        };

        let root_req = RequestNode {
            doc: HttpRequestDoc::new("req-root", "Health", HttpMethod::Get, "/health"),
            file_path: PathBuf::from("health.request.yaml"),
        };

        ProjectNode {
            doc: project_doc,
            root_dir: PathBuf::from("."),
            manifest_path: PathBuf::from("testrix.project.yaml"),
            folders: vec![parent_folder],
            requests: vec![root_req],
            environments: vec![],
        }
    }

    fn sample_env() -> EnvironmentDoc {
        let mut env = EnvironmentDoc::new("env-dev", "Development");
        env.variables = vec![
            EnvVariable::new("base_url", "https://api.example.com"),
            EnvVariable::new("project_token", "tok-proj-123").with_secret(true),
            EnvVariable::new("admin_key", "adm-456").with_secret(true),
            EnvVariable::new("expand_field", "roles"),
            EnvVariable::new("user", "alice"),
            EnvVariable {
                key: "disabled_var".to_string(),
                value: "ignored".to_string(),
                enabled: false,
                secret: false,
                description: String::new(),
            },
        ];
        env
    }

    #[test]
    fn resolves_project_to_folder_to_request_auth_inheritance() {
        let project = sample_project();

        // Root request inherits directly from Project (Bearer).
        let root_auth = resolve_request_auth(&project, "req-root").unwrap();
        assert_eq!(
            root_auth.effective,
            EffectiveAuth::Bearer {
                token: "{{project_token}}".to_string(),
                prefix: "Bearer".to_string()
            }
        );
        assert_eq!(
            root_auth.source,
            Some(AuthNodeRef::Project {
                id: "proj-1".to_string(),
                name: "Sample API".to_string()
            })
        );
        assert!(root_auth.is_inherited());

        // Leaf request walks Leaf (inherit) -> Admin (ApiKey) -> Project (Bearer), stops at Admin!
        let leaf_auth = resolve_request_auth(&project, "req-inherited").unwrap();
        assert_eq!(
            leaf_auth.effective,
            EffectiveAuth::ApiKey {
                key: "X-Admin-Key".to_string(),
                value: "{{admin_key}}".to_string(),
                location: ApiKeyLocation::Header
            }
        );
        assert_eq!(
            leaf_auth.source,
            Some(AuthNodeRef::Folder {
                id: "fld-parent".to_string(),
                name: "Admin".to_string()
            })
        );
        assert_eq!(leaf_auth.chain.len(), 4);

        // Request explicitly configured as `None` breaks inheritance from Admin/Project.
        let override_auth = resolve_request_auth(&project, "req-override").unwrap();
        assert_eq!(override_auth.effective, EffectiveAuth::None);
        assert!(!override_auth.is_inherited());
    }

    #[test]
    fn materializes_url_params_headers_auth_and_env_variables() {
        let project = sample_project();
        let env = sample_env();

        let prepared = materialize_request(&project, "req-inherited", Some(&env)).unwrap();
        assert_eq!(
            prepared.url,
            "https://api.example.com/api/v1/profile?expand=roles"
        );
        assert!(prepared.unresolved_variables.is_empty());
        assert_eq!(
            prepared.headers,
            vec![
                PreparedHeader {
                    name: "Accept".to_string(),
                    value: "application/json".to_string(),
                    secret: false,
                },
                PreparedHeader {
                    name: "X-Admin-Key".to_string(),
                    value: "adm-456".to_string(),
                    secret: true,
                },
            ]
        );
    }

    #[test]
    fn tracks_unresolved_placeholders_and_disabled_env_vars() {
        let env = sample_env();
        let res = interpolate_template("{{base_url}}/{{disabled_var}}/{{missing}}", Some(&env));
        assert_eq!(
            res.rendered,
            "https://api.example.com/{{disabled_var}}/{{missing}}"
        );
        assert_eq!(res.unresolved, vec!["disabled_var", "missing"]);
    }

    #[test]
    fn filters_explorer_preserving_ancestors() {
        let project = sample_project();
        let workspace = WorkspaceTree {
            root_dir: PathBuf::from("."),
            projects: vec![project],
            active_project_id: None,
            active_environment_id: None,
        };

        let filtered = filter_workspace(&workspace, "profile");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].requests.len(), 0);
        assert_eq!(filtered[0].folders.len(), 1);
        assert_eq!(filtered[0].folders[0].folder.doc.id, "fld-parent");
        assert_eq!(filtered[0].folders[0].folders.len(), 1);
        assert_eq!(filtered[0].folders[0].folders[0].requests.len(), 1);
        assert_eq!(
            filtered[0].folders[0].folders[0].requests[0].doc.id,
            "req-inherited"
        );
    }
}
