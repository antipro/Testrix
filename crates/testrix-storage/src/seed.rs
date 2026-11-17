use std::path::Path;

use testrix_core::{
    ApiKeyLocation, AuthConfig, EnvVariable, EnvironmentDoc, FolderDoc, HttpMethod, HttpRequestDoc,
    KeyValueRow, MultipartField, ProjectDoc, RawBodyFormat, RequestBody,
};

use crate::{error::StorageError, workspace::write_yaml_atomic};

/// Populates `workspace_root` with an example multi-folder API project conforming to `testrix.io/v1`.
pub fn seed_starter_workspace(workspace_root: &Path) -> Result<(), StorageError> {
    let project_dir = workspace_root.join("starter-api");
    let env_dir = project_dir.join("environments");
    let users_dir = project_dir.join("users");
    let admin_dir = project_dir.join("admin");
    let security_dir = admin_dir.join("security");

    std::fs::create_dir_all(&env_dir).map_err(|source| StorageError::Io {
        path: env_dir.clone(),
        source,
    })?;
    std::fs::create_dir_all(&users_dir).map_err(|source| StorageError::Io {
        path: users_dir.clone(),
        source,
    })?;
    std::fs::create_dir_all(&security_dir).map_err(|source| StorageError::Io {
        path: security_dir.clone(),
        source,
    })?;

    // Project manifest with Bearer Auth inherited by folders and requests.
    let mut project = ProjectDoc::new("proj-starter", "Starter Platform API");
    project.description =
        "Reference workspace demonstrating Testrix YAML documents, environment variables, and Project -> Folder -> Request auth inheritance."
            .to_string();
    project.base_url = "{{base_url}}".to_string();
    project.auth = AuthConfig::Bearer {
        token: "{{api_token}}".to_string(),
        prefix: "Bearer".to_string(),
    };
    project.default_headers = vec![
        KeyValueRow::new("Accept", "application/json")
            .with_description("Default JSON accept header from Project"),
        KeyValueRow::new("X-Client-App", "Testrix/0.1").with_description("Client identifier"),
    ];
    write_yaml_atomic(&project_dir.join("testrix.project.yaml"), &project)?;

    // Environments (Profiles)
    let mut dev = EnvironmentDoc::new("env-development", "Development");
    dev.description = "Local & sandbox testing profile".to_string();
    dev.variables = vec![
        EnvVariable::new("base_url", "https://httpbin.org")
            .with_description("Base HTTP echo endpoint"),
        EnvVariable::new("api_token", "dev-bearer-token-9f21a")
            .with_secret(true)
            .with_description("Bearer token used by Project auth"),
        EnvVariable::new("admin_api_key", "dev-admin-key-001")
            .with_secret(true)
            .with_description("API key overriding auth in Admin folder"),
        EnvVariable::new("sample_user", "ada.lovelace")
            .with_description("Sample username for payload templates"),
        EnvVariable::new("env_name", "development").with_description("Current environment tag"),
    ];
    write_yaml_atomic(&env_dir.join("development.env.yaml"), &dev)?;

    let mut staging = EnvironmentDoc::new("env-staging", "Staging");
    staging.description = "Pre-production verification profile".to_string();
    staging.variables = vec![
        EnvVariable::new("base_url", "https://httpbin.org").with_description("Staging endpoint"),
        EnvVariable::new("api_token", "stg-bearer-token-4b88c")
            .with_secret(true)
            .with_description("Staging Bearer token"),
        EnvVariable::new("admin_api_key", "stg-admin-key-772")
            .with_secret(true)
            .with_description("Staging Admin API key"),
        EnvVariable::new("sample_user", "grace.hopper").with_description("Sample username"),
        EnvVariable::new("env_name", "staging").with_description("Current environment tag"),
    ];
    write_yaml_atomic(&env_dir.join("staging.env.yaml"), &staging)?;

    let mut prod = EnvironmentDoc::new("env-production", "Production");
    prod.description = "Read-only production profile".to_string();
    prod.variables = vec![
        EnvVariable::new("base_url", "https://httpbin.org").with_description("Production endpoint"),
        EnvVariable::new("api_token", "prod-bearer-token-redacted")
            .with_secret(true)
            .with_description("Production Bearer token"),
        EnvVariable::new("admin_api_key", "prod-admin-key-redacted")
            .with_secret(true)
            .with_description("Production Admin API key"),
        EnvVariable::new("sample_user", "alan.turing").with_description("Sample username"),
        EnvVariable::new("env_name", "production").with_description("Current environment tag"),
    ];
    write_yaml_atomic(&env_dir.join("production.env.yaml"), &prod)?;

    // Root-level request (inherits Project Bearer auth)
    let mut inspect_request = HttpRequestDoc::new(
        "req-inspect-get",
        "Inspect Request Headers",
        HttpMethod::Get,
        "/get",
    );
    inspect_request.order = 1;
    inspect_request.description =
        "Echoes request headers and query args; inherits Bearer auth directly from Project."
            .to_string();
    inspect_request.request.params = vec![
        KeyValueRow::new("env", "{{env_name}}").with_description("Active environment profile"),
        KeyValueRow::new("debug", "true").with_enabled(false),
    ];
    write_yaml_atomic(
        &project_dir.join("inspect-get.request.yaml"),
        &inspect_request,
    )?;

    // Users folder (inherits Project Bearer auth)
    let mut users_folder = FolderDoc::new("fld-users", "Users");
    users_folder.order = 10;
    users_folder.description =
        "Inherits Bearer authentication from Starter Platform API.".to_string();
    users_folder.auth = AuthConfig::Inherit;
    write_yaml_atomic(&users_dir.join("_folder.yaml"), &users_folder)?;

    let mut list_users = HttpRequestDoc::new(
        "req-users-list",
        "List Users",
        HttpMethod::Get,
        "/anything/users",
    );
    list_users.order = 10;
    list_users.description = "Paginated user listing inheriting Project Bearer auth.".to_string();
    list_users.request.params = vec![
        KeyValueRow::new("page", "1").with_description("Page number"),
        KeyValueRow::new("limit", "25").with_description("Page size"),
    ];
    write_yaml_atomic(&users_dir.join("list-users.request.yaml"), &list_users)?;

    let mut create_user =
        HttpRequestDoc::new("req-users-create", "Create User", HttpMethod::Post, "/post");
    create_user.order = 20;
    create_user.description = "Creates a user with a JSON payload.".to_string();
    create_user.request.headers = vec![
        KeyValueRow::new("X-Idempotency-Key", "create-{{sample_user}}-001")
            .with_description("Idempotency token"),
    ];
    create_user.request.body = RequestBody::Json {
        content: "{\n  \"username\": \"{{sample_user}}\",\n  \"environment\": \"{{env_name}}\",\n  \"role\": \"maintainer\"\n}".to_string(),
    };
    write_yaml_atomic(&users_dir.join("create-user.request.yaml"), &create_user)?;

    let mut update_form = HttpRequestDoc::new(
        "req-users-update-form",
        "Update Profile (Form)",
        HttpMethod::Put,
        "/put",
    );
    update_form.order = 30;
    update_form.description = "Demonstrates application/x-www-form-urlencoded payload.".to_string();
    update_form.request.body = RequestBody::FormUrlEncoded {
        fields: vec![
            KeyValueRow::new("username", "{{sample_user}}"),
            KeyValueRow::new("timezone", "UTC"),
        ],
    };
    write_yaml_atomic(
        &users_dir.join("update-profile-form.request.yaml"),
        &update_form,
    )?;

    let mut upload_multipart = HttpRequestDoc::new(
        "req-users-upload-multipart",
        "Upload Metadata (Multipart)",
        HttpMethod::Patch,
        "/patch",
    );
    upload_multipart.order = 40;
    upload_multipart.description = "Demonstrates multipart/form-data payload.".to_string();
    upload_multipart.request.body = RequestBody::Multipart {
        fields: vec![
            MultipartField::text("user", "{{sample_user}}"),
            MultipartField::text("env", "{{env_name}}"),
        ],
    };
    write_yaml_atomic(
        &users_dir.join("upload-metadata-multipart.request.yaml"),
        &upload_multipart,
    )?;

    // Admin folder (overrides auth to API Key!)
    let mut admin_folder = FolderDoc::new("fld-admin", "Admin");
    admin_folder.order = 20;
    admin_folder.description =
        "Overrides Project auth with an Admin API Key header inherited by nested folders and requests."
            .to_string();
    admin_folder.auth = AuthConfig::ApiKey {
        key: "X-Admin-Key".to_string(),
        value: "{{admin_api_key}}".to_string(),
        location: ApiKeyLocation::Header,
    };
    write_yaml_atomic(&admin_dir.join("_folder.yaml"), &admin_folder)?;

    let mut audit_logs = HttpRequestDoc::new(
        "req-admin-audit",
        "Fetch Audit Logs",
        HttpMethod::Get,
        "/headers",
    );
    audit_logs.order = 10;
    audit_logs.description = "Inherits X-Admin-Key header from Admin folder.".to_string();
    write_yaml_atomic(
        &admin_dir.join("fetch-audit-logs.request.yaml"),
        &audit_logs,
    )?;

    // Nested Security folder (inherits Admin folder's API Key!)
    let mut security_folder = FolderDoc::new("fld-admin-security", "Security");
    security_folder.order = 10;
    security_folder.description =
        "Nested folder set to Inherit — receives X-Admin-Key from Admin parent folder.".to_string();
    security_folder.auth = AuthConfig::Inherit;
    write_yaml_atomic(&security_dir.join("_folder.yaml"), &security_folder)?;

    let mut rotate_keys = HttpRequestDoc::new(
        "req-security-rotate",
        "Rotate Signing Keys",
        HttpMethod::Delete,
        "/delete",
    );
    rotate_keys.order = 10;
    rotate_keys.description =
        "Multi-hop auth inheritance: Request -> Security -> Admin (X-Admin-Key).".to_string();
    rotate_keys.request.body = RequestBody::Raw {
        raw_format: RawBodyFormat::Yaml,
        content: "reason: scheduled-rotation\nenvironment: \"{{env_name}}\"\n".to_string(),
    };
    write_yaml_atomic(
        &security_dir.join("rotate-signing-keys.request.yaml"),
        &rotate_keys,
    )?;

    let mut public_ping = HttpRequestDoc::new(
        "req-security-public-ping",
        "Public Health Check",
        HttpMethod::Get,
        "/status/200",
    );
    public_ping.order = 20;
    public_ping.description =
        "Explicitly sets No Auth to break folder/project inheritance.".to_string();
    public_ping.request.auth = AuthConfig::None;
    write_yaml_atomic(
        &security_dir.join("public-health-check.request.yaml"),
        &public_ping,
    )?;

    Ok(())
}
