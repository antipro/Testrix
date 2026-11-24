use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::CoreError,
    response_meta::{ResponseExample, ResponseMetaField},
};

/// Canonical `apiVersion` for all Testrix YAML documents.
pub const API_VERSION: &str = "testrix.io/v1";

/// Generates a new stable domain identifier with the given prefix.
pub fn generate_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

/// Discriminator identifying the YAML document kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentKind {
    Project,
    Folder,
    HttpRequest,
    Environment,
}

/// Supported HTTP methods.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    /// Ordered list of all supported HTTP methods.
    pub const ALL: [Self; 7] = [
        Self::Get,
        Self::Post,
        Self::Put,
        Self::Delete,
        Self::Patch,
        Self::Head,
        Self::Options,
    ];

    /// Canonical uppercase HTTP method name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }

    /// Compact method label for explorer rows and workbench tabs.
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DEL",
            Self::Patch => "PAT",
            Self::Head => "HEAD",
            Self::Options => "OPT",
        }
    }

    /// Parse a case-insensitive HTTP method string.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "DELETE" => Some(Self::Delete),
            "PATCH" => Some(Self::Patch),
            "HEAD" => Some(Self::Head),
            "OPTIONS" => Some(Self::Options),
            _ => None,
        }
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A toggleable key-value entry used for query parameters, headers, and form fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct KeyValueRow {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl KeyValueRow {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
            description: String::new(),
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

impl Default for KeyValueRow {
    fn default() -> Self {
        Self::new("", "")
    }
}

/// Where an API key credential is injected into an outgoing HTTP request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
}

impl ApiKeyLocation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Header => "Header",
            Self::Query => "Query Param",
        }
    }
}

/// Authentication configuration attached to a Project, Folder, or HTTP Request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthConfig {
    #[default]
    Inherit,
    None,
    Bearer {
        #[serde(default)]
        token: String,
        #[serde(default = "default_bearer_prefix")]
        prefix: String,
    },
    Basic {
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
    },
    ApiKey {
        #[serde(default)]
        key: String,
        #[serde(default)]
        value: String,
        #[serde(default, rename = "in")]
        location: ApiKeyLocation,
    },
}

impl AuthConfig {
    /// Human-readable short mode label.
    pub const fn mode_label(&self) -> &'static str {
        match self {
            Self::Inherit => "Inherit from parent",
            Self::None => "No Auth",
            Self::Bearer { .. } => "Bearer Token",
            Self::Basic { .. } => "Basic Auth",
            Self::ApiKey { .. } => "API Key",
        }
    }

    /// Returns true if this node explicitly configures non-inherited authentication.
    pub const fn is_explicit(&self) -> bool {
        !matches!(self, Self::Inherit)
    }

    /// Converts an `AuthConfig` to an `EffectiveAuth`, treating `Inherit` as `None`.
    pub fn into_effective(&self) -> EffectiveAuth {
        match self {
            Self::Inherit | Self::None => EffectiveAuth::None,
            Self::Bearer { token, prefix } => EffectiveAuth::Bearer {
                token: token.clone(),
                prefix: if prefix.trim().is_empty() {
                    default_bearer_prefix()
                } else {
                    prefix.clone()
                },
            },
            Self::Basic { username, password } => EffectiveAuth::Basic {
                username: username.clone(),
                password: password.clone(),
            },
            Self::ApiKey {
                key,
                value,
                location,
            } => EffectiveAuth::ApiKey {
                key: key.clone(),
                value: value.clone(),
                location: *location,
            },
        }
    }
}

/// Concrete authentication resolved after walking the inheritance chain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EffectiveAuth {
    #[default]
    None,
    Bearer {
        token: String,
        prefix: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        key: String,
        value: String,
        location: ApiKeyLocation,
    },
}

impl EffectiveAuth {
    pub const fn mode_label(&self) -> &'static str {
        match self {
            Self::None => "No Auth",
            Self::Bearer { .. } => "Bearer Token",
            Self::Basic { .. } => "Basic Auth",
            Self::ApiKey { .. } => "API Key",
        }
    }
}

/// Sub-format for `RequestBody::Raw`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RawBodyFormat {
    #[default]
    Text,
    Xml,
    Html,
    Javascript,
    Yaml,
}

impl RawBodyFormat {
    pub const ALL: [Self; 5] = [
        Self::Text,
        Self::Xml,
        Self::Html,
        Self::Javascript,
        Self::Yaml,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Text => "Plain Text",
            Self::Xml => "XML",
            Self::Html => "HTML",
            Self::Javascript => "JavaScript",
            Self::Yaml => "YAML",
        }
    }

    pub const fn default_content_type(self) -> &'static str {
        match self {
            Self::Text => "text/plain; charset=utf-8",
            Self::Xml => "application/xml",
            Self::Html => "text/html; charset=utf-8",
            Self::Javascript => "application/javascript",
            Self::Yaml => "application/yaml",
        }
    }

    pub const fn editor_language(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Xml | Self::Html => "html",
            Self::Javascript => "text",
            Self::Yaml => "yaml",
        }
    }
}

/// Field kind for `multipart/form-data` entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MultipartFieldType {
    #[default]
    Text,
    File,
}

/// A single entry in a `multipart/form-data` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MultipartField {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub field_type: MultipartFieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl MultipartField {
    pub fn text(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            field_type: MultipartFieldType::Text,
            file_name: None,
            content_type: None,
            enabled: true,
            description: String::new(),
        }
    }

    pub fn file(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: path.into(),
            field_type: MultipartFieldType::File,
            file_name: None,
            content_type: None,
            enabled: true,
            description: String::new(),
        }
    }
}

impl Default for MultipartField {
    fn default() -> Self {
        Self::text("", "")
    }
}

/// Request payload configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RequestBody {
    #[default]
    None,
    Json {
        #[serde(default)]
        content: String,
    },
    Raw {
        #[serde(default, rename = "rawFormat")]
        raw_format: RawBodyFormat,
        #[serde(default)]
        content: String,
    },
    FormUrlEncoded {
        #[serde(default)]
        fields: Vec<KeyValueRow>,
    },
    Multipart {
        #[serde(default)]
        fields: Vec<MultipartField>,
    },
    Binary {
        #[serde(default, rename = "filePath")]
        file_path: String,
        #[serde(
            default,
            rename = "contentType",
            skip_serializing_if = "Option::is_none"
        )]
        content_type: Option<String>,
    },
}

impl RequestBody {
    pub const fn mode_label(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Json { .. } => "JSON",
            Self::Raw { .. } => "Raw",
            Self::FormUrlEncoded { .. } => "Form URL-Encoded",
            Self::Multipart { .. } => "Multipart",
            Self::Binary { .. } => "Binary",
        }
    }
}

/// Per-request execution settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct RequestSettings {
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_true")]
    pub follow_redirects: bool,
    #[serde(default = "default_true")]
    pub verify_tls: bool,
}

impl RequestSettings {
    pub fn new(timeout_ms: u64, follow_redirects: bool, verify_tls: bool) -> Self {
        Self {
            timeout_ms,
            follow_redirects,
            verify_tls,
        }
    }
}

impl Default for RequestSettings {
    fn default() -> Self {
        Self {
            timeout_ms: default_timeout_ms(),
            follow_redirects: true,
            verify_tls: true,
        }
    }
}

/// HTTP request details nested under `HttpRequestDoc::request`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct HttpRequestSpec {
    #[serde(default)]
    pub method: HttpMethod,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<KeyValueRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_params: Vec<KeyValueRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValueRow>,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub body: RequestBody,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub post_script: String,
    #[serde(default)]
    pub settings: RequestSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_meta: Vec<ResponseMetaField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_examples: Vec<ResponseExample>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_meta: Vec<ResponseMetaField>,
}

impl Default for HttpRequestSpec {
    fn default() -> Self {
        Self {
            method: HttpMethod::Get,
            url: String::new(),
            params: Vec::new(),
            path_params: Vec::new(),
            headers: Vec::new(),
            auth: AuthConfig::Inherit,
            body: RequestBody::None,
            post_script: String::new(),
            settings: RequestSettings::default(),
            body_meta: Vec::new(),
            response_examples: Vec::new(),
            response_meta: Vec::new(),
        }
    }
}

/// Top-level YAML document for an HTTP request (`*.request.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct HttpRequestDoc {
    pub api_version: String,
    pub kind: DocumentKind,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub request: HttpRequestSpec,
}

impl HttpRequestDoc {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        method: HttpMethod,
        url: impl Into<String>,
    ) -> Self {
        let request = HttpRequestSpec {
            method,
            url: url.into(),
            ..Default::default()
        };
        Self {
            api_version: API_VERSION.to_string(),
            kind: DocumentKind::HttpRequest,
            id: id.into(),
            name: name.into(),
            description: String::new(),
            order: 0,
            request,
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        validate_envelope(
            &self.api_version,
            self.kind,
            DocumentKind::HttpRequest,
            &self.id,
            &self.name,
        )?;
        if self.request.settings.timeout_ms == 0 {
            return Err(CoreError::Validation {
                reason: format!("request `{}` timeoutMs must be >= 1", self.id),
            });
        }
        Ok(())
    }
}

/// A recorded HTTP request execution entry shown in the History view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestHistoryEntry {
    pub id: String,
    pub executed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_id: Option<String>,
    pub doc: HttpRequestDoc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// A reference to an existing HTTP request stored inside a [`RequestSet`] or [`RequestSetFolder`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSetItem {
    pub id: String,
    pub project_id: String,
    pub request_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub method: HttpMethod,
    #[serde(default)]
    pub url: String,
}

/// A folder inside a [`RequestSet`] that can contain subfolders and request references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSetFolder {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<RequestSetFolder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<RequestSetItem>,
}

/// A named group of HTTP request references with nested folders and an optional execution DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSet {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<RequestSetFolder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<RequestSetItem>,
    #[serde(default, skip_serializing_if = "crate::dag::PackageDag::is_empty")]
    pub dag: crate::dag::PackageDag,
}

/// Top-level YAML document for a folder (`_folder.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct FolderDoc {
    pub api_version: String,
    pub kind: DocumentKind,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub auth: AuthConfig,
}

impl FolderDoc {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            kind: DocumentKind::Folder,
            id: id.into(),
            name: name.into(),
            description: String::new(),
            order: 0,
            auth: AuthConfig::Inherit,
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        validate_envelope(
            &self.api_version,
            self.kind,
            DocumentKind::Folder,
            &self.id,
            &self.name,
        )
    }
}

/// Top-level YAML document for a project (`testrix.project.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ProjectDoc {
    pub api_version: String,
    pub kind: DocumentKind,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(default = "default_project_auth")]
    pub auth: AuthConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_headers: Vec<KeyValueRow>,
}

impl ProjectDoc {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            kind: DocumentKind::Project,
            id: id.into(),
            name: name.into(),
            description: String::new(),
            base_url: String::new(),
            auth: AuthConfig::None,
            default_headers: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        validate_envelope(
            &self.api_version,
            self.kind,
            DocumentKind::Project,
            &self.id,
            &self.name,
        )
    }
}

/// Single variable inside an Environment profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EnvVariable {
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl EnvVariable {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            enabled: true,
            secret: false,
            description: String::new(),
        }
    }

    pub fn with_secret(mut self, secret: bool) -> Self {
        self.secret = secret;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// Character category rank for sorting environment variable keys:
/// `0` = special symbol, `1` = number, `2` = alphabet.
fn env_key_char_rank(c: char) -> u8 {
    if c.is_ascii_digit() || c.is_numeric() {
        1
    } else if c.is_alphabetic() {
        2
    } else {
        0
    }
}

/// Compares two environment variable keys by: special symbol -> number -> alphabet.
pub fn compare_env_var_keys(a: &str, b: &str) -> std::cmp::Ordering {
    let mut chars_a = a.trim().chars();
    let mut chars_b = b.trim().chars();
    loop {
        match (chars_a.next(), chars_b.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(ca), Some(cb)) => {
                let ra = env_key_char_rank(ca);
                let rb = env_key_char_rank(cb);
                if ra != rb {
                    return ra.cmp(&rb);
                }
                let ord = if ra == 2 {
                    ca.to_lowercase()
                        .cmp(cb.to_lowercase())
                        .then_with(|| ca.cmp(&cb))
                } else {
                    ca.cmp(&cb)
                };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Sorts environment variables in-place by key: special symbol, number, then alphabet.
pub fn sort_env_variables(vars: &mut [EnvVariable]) {
    vars.sort_by(|a, b| compare_env_var_keys(&a.key, &b.key));
}

impl Default for EnvVariable {
    fn default() -> Self {
        Self::new("", "")
    }
}

/// Top-level YAML document for an environment profile (`environments/*.env.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EnvironmentDoc {
    pub api_version: String,
    pub kind: DocumentKind,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub variables: Vec<EnvVariable>,
}

impl EnvironmentDoc {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            kind: DocumentKind::Environment,
            id: id.into(),
            name: name.into(),
            description: String::new(),
            variables: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        validate_envelope(
            &self.api_version,
            self.kind,
            DocumentKind::Environment,
            &self.id,
            &self.name,
        )
    }
}

/// Loaded request node with its backing YAML file path on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestNode {
    pub doc: HttpRequestDoc,
    pub file_path: PathBuf,
}

/// Loaded folder node with its children and backing directory/manifest paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderNode {
    pub doc: FolderDoc,
    pub dir_path: PathBuf,
    pub manifest_path: PathBuf,
    pub has_manifest: bool,
    pub folders: Vec<FolderNode>,
    pub requests: Vec<RequestNode>,
}

/// Loaded environment profile node with its backing YAML file path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentNode {
    pub doc: EnvironmentDoc,
    pub file_path: PathBuf,
}

/// Loaded project root node containing folders, root requests, and environments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectNode {
    pub doc: ProjectDoc,
    pub root_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub folders: Vec<FolderNode>,
    pub requests: Vec<RequestNode>,
    pub environments: Vec<EnvironmentNode>,
}

/// Reserved profile ID for the workspace-wide Global environment (`environments/_global.env.yaml`).
pub const GLOBAL_ENV_ID: &str = "__global__";

/// Complete loaded workspace containing projects, shared environments, and active UI selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceTree {
    pub root_dir: PathBuf,
    pub projects: Vec<ProjectNode>,
    pub environments: Vec<EnvironmentNode>,
    pub global_environment: Option<EnvironmentNode>,
    pub active_project_id: Option<String>,
    pub active_environment_id: Option<String>,
}

/// Reference to a node found in a [`WorkspaceTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplorerNode {
    Project {
        project_id: String,
    },
    Folder {
        project_id: String,
        folder_id: String,
    },
    Request {
        project_id: String,
        request_id: String,
    },
}

impl WorkspaceTree {
    /// Returns the currently active project, falling back to the first project.
    pub fn active_project(&self) -> Option<&ProjectNode> {
        if let Some(id) = &self.active_project_id
            && let Some(project) = self.projects.iter().find(|p| &p.doc.id == id)
        {
            return Some(project);
        }
        self.projects.first()
    }

    /// Returns the currently active project mutably.
    pub fn active_project_mut(&mut self) -> Option<&mut ProjectNode> {
        let target_id = self
            .active_project_id
            .as_ref()
            .filter(|id| self.projects.iter().any(|p| &p.doc.id == *id))
            .cloned()
            .or_else(|| self.projects.first().map(|p| p.doc.id.clone()));

        let id = target_id?;
        self.projects.iter_mut().find(|p| p.doc.id == id)
    }

    /// Returns the currently active workspace-wide environment document.
    pub fn active_environment(&self) -> Option<&EnvironmentNode> {
        let env_id = self.active_environment_id.as_deref()?;
        self.environments
            .iter()
            .find(|env| env.doc.id == env_id)
            .or_else(|| {
                self.active_project()?
                    .environments
                    .iter()
                    .find(|env| env.doc.id == env_id)
            })
    }

    /// Returns an effective [`EnvironmentDoc`] combining the Global environment (base) with the
    /// currently active profile (override).
    pub fn effective_environment(&self) -> Option<EnvironmentDoc> {
        let global = self.global_environment.as_ref().map(|e| &e.doc);
        let active = self.active_environment().map(|e| &e.doc);
        match (global, active) {
            (None, None) => None,
            (Some(g), None) => Some(g.clone()),
            (None, Some(a)) => Some(a.clone()),
            (Some(g), Some(a)) => {
                let mut merged = a.clone();
                let mut existing_keys: std::collections::BTreeSet<String> = merged
                    .variables
                    .iter()
                    .filter(|v| v.enabled && !v.key.trim().is_empty())
                    .map(|v| v.key.trim().to_string())
                    .collect();
                for gv in &g.variables {
                    let key = gv.key.trim();
                    if gv.enabled && !key.is_empty() && existing_keys.insert(key.to_string()) {
                        merged.variables.push(gv.clone());
                    }
                }
                Some(merged)
            }
        }
    }

    /// Finds a project by ID.
    pub fn find_project(&self, project_id: &str) -> Option<&ProjectNode> {
        self.projects.iter().find(|p| p.doc.id == project_id)
    }

    /// Finds a project mutably by ID.
    pub fn find_project_mut(&mut self, project_id: &str) -> Option<&mut ProjectNode> {
        self.projects.iter_mut().find(|p| p.doc.id == project_id)
    }

    /// Finds a request across all projects, returning `(project, folder_ancestors, request)`.
    pub fn find_request(
        &self,
        request_id: &str,
    ) -> Option<(&ProjectNode, Vec<&FolderNode>, &RequestNode)> {
        for project in &self.projects {
            if let Some((ancestors, req)) = project.find_request(request_id) {
                return Some((project, ancestors, req));
            }
        }
        None
    }

    /// Finds a folder across all projects, returning `(project, ancestors_including_target)`.
    pub fn find_folder(&self, folder_id: &str) -> Option<(&ProjectNode, Vec<&FolderNode>)> {
        for project in &self.projects {
            if let Some(chain) = project.find_folder(folder_id) {
                return Some((project, chain));
            }
        }
        None
    }

    /// Looks up a node by tree item ID (`proj:<id>`, `fld:<id>`, `req:<id>`).
    pub fn lookup_tree_id(&self, tree_id: &str) -> Option<ExplorerNode> {
        if let Some(id) = tree_id.strip_prefix("proj:") {
            self.find_project(id).map(|_| ExplorerNode::Project {
                project_id: id.to_string(),
            })
        } else if let Some(id) = tree_id.strip_prefix("fld:") {
            self.find_folder(id).map(|(p, _)| ExplorerNode::Folder {
                project_id: p.doc.id.clone(),
                folder_id: id.to_string(),
            })
        } else if let Some(id) = tree_id.strip_prefix("req:") {
            self.find_request(id)
                .map(|(p, _, _)| ExplorerNode::Request {
                    project_id: p.doc.id.clone(),
                    request_id: id.to_string(),
                })
        } else {
            None
        }
    }

    /// Total number of requests across all projects.
    pub fn total_request_count(&self) -> usize {
        self.projects.iter().map(ProjectNode::request_count).sum()
    }
}

impl ProjectNode {
    /// Finds a request by ID and returns its folder ancestor chain (outermost to innermost).
    pub fn find_request(&self, request_id: &str) -> Option<(Vec<&FolderNode>, &RequestNode)> {
        if let Some(req) = self.requests.iter().find(|r| r.doc.id == request_id) {
            return Some((Vec::new(), req));
        }
        for folder in &self.folders {
            if let Some((mut path, req)) = find_request_in_folder(folder, request_id) {
                path.insert(0, folder);
                return Some((path, req));
            }
        }
        None
    }

    /// Finds a folder by ID and returns its chain from root folder down to the target folder.
    pub fn find_folder(&self, folder_id: &str) -> Option<Vec<&FolderNode>> {
        for folder in &self.folders {
            if let Some(mut chain) = find_folder_chain(folder, folder_id) {
                chain.insert(0, folder);
                return Some(chain);
            }
        }
        None
    }

    /// Finds a mutable request by ID anywhere in the project.
    pub fn find_request_mut(&mut self, request_id: &str) -> Option<&mut RequestNode> {
        if let Some(req) = self.requests.iter_mut().find(|r| r.doc.id == request_id) {
            return Some(req);
        }
        for folder in &mut self.folders {
            if let Some(req) = find_request_in_folder_mut(folder, request_id) {
                return Some(req);
            }
        }
        None
    }

    /// Finds a mutable folder by ID anywhere in the project.
    pub fn find_folder_mut(&mut self, folder_id: &str) -> Option<&mut FolderNode> {
        for folder in &mut self.folders {
            if let Some(found) = find_folder_mut_inner(folder, folder_id) {
                return Some(found);
            }
        }
        None
    }

    /// Total number of requests in this project.
    pub fn request_count(&self) -> usize {
        self.requests.len()
            + self
                .folders
                .iter()
                .map(FolderNode::request_count)
                .sum::<usize>()
    }
}

impl FolderNode {
    pub fn request_count(&self) -> usize {
        self.requests.len() + self.folders.iter().map(Self::request_count).sum::<usize>()
    }
}

fn find_request_in_folder<'a>(
    folder: &'a FolderNode,
    request_id: &str,
) -> Option<(Vec<&'a FolderNode>, &'a RequestNode)> {
    if let Some(req) = folder.requests.iter().find(|r| r.doc.id == request_id) {
        return Some((Vec::new(), req));
    }
    for sub in &folder.folders {
        if let Some((mut path, req)) = find_request_in_folder(sub, request_id) {
            path.insert(0, sub);
            return Some((path, req));
        }
    }
    None
}

fn find_folder_chain<'a>(folder: &'a FolderNode, target_id: &str) -> Option<Vec<&'a FolderNode>> {
    if folder.doc.id == target_id {
        return Some(Vec::new());
    }
    for sub in &folder.folders {
        if let Some(mut path) = find_folder_chain(sub, target_id) {
            path.insert(0, sub);
            return Some(path);
        }
    }
    None
}

fn find_request_in_folder_mut<'a>(
    folder: &'a mut FolderNode,
    request_id: &str,
) -> Option<&'a mut RequestNode> {
    if let Some(req) = folder.requests.iter_mut().find(|r| r.doc.id == request_id) {
        return Some(req);
    }
    for sub in &mut folder.folders {
        if let Some(req) = find_request_in_folder_mut(sub, request_id) {
            return Some(req);
        }
    }
    None
}

fn find_folder_mut_inner<'a>(
    folder: &'a mut FolderNode,
    target_id: &str,
) -> Option<&'a mut FolderNode> {
    if folder.doc.id == target_id {
        return Some(folder);
    }
    for sub in &mut folder.folders {
        if let Some(found) = find_folder_mut_inner(sub, target_id) {
            return Some(found);
        }
    }
    None
}

fn validate_envelope(
    api_version: &str,
    actual_kind: DocumentKind,
    expected_kind: DocumentKind,
    id: &str,
    name: &str,
) -> Result<(), CoreError> {
    if api_version != API_VERSION {
        return Err(CoreError::Validation {
            reason: format!("unsupported apiVersion `{api_version}`, expected `{API_VERSION}`"),
        });
    }
    if actual_kind != expected_kind {
        return Err(CoreError::Validation {
            reason: format!("unexpected kind `{actual_kind:?}`, expected `{expected_kind:?}`"),
        });
    }
    if id.trim().is_empty() {
        return Err(CoreError::Validation {
            reason: "document id must not be empty".to_string(),
        });
    }
    if name.trim().is_empty() {
        return Err(CoreError::Validation {
            reason: format!("document `{id}` name must not be empty"),
        });
    }
    Ok(())
}

const fn default_true() -> bool {
    true
}

const fn default_timeout_ms() -> u64 {
    30_000
}

fn default_bearer_prefix() -> String {
    "Bearer".to_string()
}

fn default_project_auth() -> AuthConfig {
    AuthConfig::None
}
