use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use url::Url;

use crate::{
    auth::{ResolvedAuth, resolve_request_doc_auth},
    error::CoreError,
    model::{
        ApiKeyLocation, EffectiveAuth, EnvironmentDoc, FolderNode, HttpMethod, HttpRequestDoc,
        MultipartFieldType, ProjectNode, RequestBody, RequestSettings,
    },
    template::{env_variable_map, interpolate_with_map},
};

/// Fully resolved HTTP header ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedHeader {
    pub name: String,
    pub value: String,
    pub secret: bool,
}

/// Multipart part resolved with environment interpolation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedMultipartPart {
    Text {
        name: String,
        value: String,
        content_type: Option<String>,
    },
    File {
        name: String,
        path: String,
        file_name: Option<String>,
        content_type: Option<String>,
    },
}

/// Resolved request body ready for `testrix-http`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedBody {
    None,
    Text {
        content: String,
    },
    FormUrlEncoded {
        fields: Vec<(String, String)>,
    },
    Multipart {
        parts: Vec<PreparedMultipartPart>,
    },
    Binary {
        file_path: String,
        content_type: Option<String>,
    },
}

impl PreparedBody {
    pub fn preview(&self) -> String {
        match self {
            Self::None => "(empty)".to_string(),
            Self::Text { content } => content.clone(),
            Self::FormUrlEncoded { fields } => fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&"),
            Self::Multipart { parts } => parts
                .iter()
                .map(|part| match part {
                    PreparedMultipartPart::Text { name, value, .. } => format!("{name}: {value}"),
                    PreparedMultipartPart::File { name, path, .. } => {
                        format!("{name}: <file:{path}>")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Binary {
                file_path,
                content_type,
            } => format!(
                "<binary:{}> ({})",
                file_path,
                content_type
                    .as_deref()
                    .unwrap_or("application/octet-stream")
            ),
        }
    }
}

/// Fully materialized HTTP request ready to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    pub request_id: String,
    pub name: String,
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<PreparedHeader>,
    pub body: PreparedBody,
    pub settings: RequestSettings,
    pub resolved_auth: ResolvedAuth,
    pub unresolved_variables: Vec<String>,
}

/// Materializes a saved request by ID from `project`.
pub fn materialize_request(
    project: &ProjectNode,
    request_id: &str,
    env: Option<&EnvironmentDoc>,
) -> Result<PreparedRequest, CoreError> {
    let (ancestors, req) =
        project
            .find_request(request_id)
            .ok_or_else(|| CoreError::RequestNotFound {
                request_id: request_id.to_string(),
            })?;
    materialize_request_doc(project, &ancestors, &req.doc, env)
}

/// Materializes an [`HttpRequestDoc`] (including unsaved workbench edits) with inheritance and variables.
pub fn materialize_request_doc(
    project: &ProjectNode,
    ancestors: &[&FolderNode],
    doc: &HttpRequestDoc,
    env: Option<&EnvironmentDoc>,
) -> Result<PreparedRequest, CoreError> {
    let vars = env_variable_map(env);
    let mut unresolved = Vec::new();
    let expand = |raw: &str, unresolved: &mut Vec<String>| -> String {
        let res = interpolate_with_map(raw, &vars);
        for var in res.unresolved {
            if !unresolved.iter().any(|existing| existing == &var) {
                unresolved.push(var);
            }
        }
        res.rendered
    };

    let raw_url = expand(&doc.request.url, &mut unresolved);
    let base_url = expand(&project.doc.base_url, &mut unresolved);
    let joined_url = join_base_and_path(&base_url, &raw_url);
    if joined_url.trim().is_empty() {
        return Err(CoreError::InvalidUrl {
            url: joined_url,
            reason: "URL must not be empty".to_string(),
        });
    }

    let mut parsed_url = Url::parse(&joined_url).map_err(|err| CoreError::InvalidUrl {
        url: joined_url.clone(),
        reason: err.to_string(),
    })?;

    // Collect headers case-insensitively while preserving insertion order.
    let mut headers: Vec<PreparedHeader> = Vec::new();

    for row in &project.doc.default_headers {
        if row.enabled && !row.name.trim().is_empty() {
            upsert_header(
                &mut headers,
                expand(&row.name, &mut unresolved),
                expand(&row.value, &mut unresolved),
                false,
            );
        }
    }

    if let Some(default_ct) = default_content_type(&doc.request.body) {
        upsert_header_if_missing(&mut headers, "Content-Type", default_ct, false);
    }

    let resolved_auth = resolve_request_doc_auth(project, ancestors, doc);
    let mut extra_query: Vec<(String, String)> = Vec::new();
    apply_effective_auth(
        &resolved_auth.effective,
        &vars,
        &mut unresolved,
        &mut headers,
        &mut extra_query,
    );

    for row in &doc.request.headers {
        if row.enabled && !row.name.trim().is_empty() {
            upsert_header(
                &mut headers,
                expand(&row.name, &mut unresolved),
                expand(&row.value, &mut unresolved),
                false,
            );
        }
    }

    // Append enabled query parameters and any API-key query parameters.
    {
        let mut query = parsed_url.query_pairs_mut();
        for param in &doc.request.params {
            if param.enabled && !param.name.trim().is_empty() {
                query.append_pair(
                    &expand(&param.name, &mut unresolved),
                    &expand(&param.value, &mut unresolved),
                );
            }
        }
        for (k, v) in extra_query {
            query.append_pair(&k, &v);
        }
    }

    // Drop trailing '?' if no query params were actually present.
    if parsed_url.query().is_some_and(str::is_empty) {
        parsed_url.set_query(None);
    }

    let body = match &doc.request.body {
        RequestBody::None => PreparedBody::None,
        RequestBody::Json { content } => PreparedBody::Text {
            content: expand(content, &mut unresolved),
        },
        RequestBody::Raw { content, .. } => PreparedBody::Text {
            content: expand(content, &mut unresolved),
        },
        RequestBody::FormUrlEncoded { fields } => PreparedBody::FormUrlEncoded {
            fields: fields
                .iter()
                .filter(|row| row.enabled && !row.name.trim().is_empty())
                .map(|row| {
                    (
                        expand(&row.name, &mut unresolved),
                        expand(&row.value, &mut unresolved),
                    )
                })
                .collect(),
        },
        RequestBody::Multipart { fields } => PreparedBody::Multipart {
            parts: fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
                .map(|field| {
                    let name = expand(&field.name, &mut unresolved);
                    let value = expand(&field.value, &mut unresolved);
                    let content_type = field
                        .content_type
                        .as_deref()
                        .map(|ct| expand(ct, &mut unresolved))
                        .filter(|ct| !ct.trim().is_empty());
                    match field.field_type {
                        MultipartFieldType::Text => PreparedMultipartPart::Text {
                            name,
                            value,
                            content_type,
                        },
                        MultipartFieldType::File => PreparedMultipartPart::File {
                            name,
                            path: value,
                            file_name: field
                                .file_name
                                .as_deref()
                                .map(|s| expand(s, &mut unresolved))
                                .filter(|s| !s.trim().is_empty()),
                            content_type,
                        },
                    }
                })
                .collect(),
        },
        RequestBody::Binary {
            file_path,
            content_type,
        } => PreparedBody::Binary {
            file_path: expand(file_path, &mut unresolved),
            content_type: content_type
                .as_deref()
                .map(|ct| expand(ct, &mut unresolved))
                .filter(|ct| !ct.trim().is_empty()),
        },
    };

    Ok(PreparedRequest {
        request_id: doc.id.clone(),
        name: doc.name.clone(),
        method: doc.request.method,
        url: parsed_url.to_string(),
        headers,
        body,
        settings: doc.request.settings.clone(),
        resolved_auth,
        unresolved_variables: unresolved,
    })
}

fn join_base_and_path(base_url: &str, request_url: &str) -> String {
    let trimmed = request_url.trim();
    if trimmed.starts_with('/') && !base_url.trim().is_empty() {
        let base = base_url.trim().trim_end_matches('/');
        format!("{base}{trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn default_content_type(body: &RequestBody) -> Option<String> {
    match body {
        RequestBody::Json { .. } => Some("application/json".to_string()),
        RequestBody::Raw { raw_format, .. } => Some(raw_format.default_content_type().to_string()),
        RequestBody::FormUrlEncoded { .. } => Some("application/x-www-form-urlencoded".to_string()),
        RequestBody::Binary { content_type, .. } => Some(
            content_type
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("application/octet-stream")
                .to_string(),
        ),
        RequestBody::None | RequestBody::Multipart { .. } => None,
    }
}

fn apply_effective_auth(
    auth: &EffectiveAuth,
    vars: &BTreeMap<String, String>,
    unresolved: &mut Vec<String>,
    headers: &mut Vec<PreparedHeader>,
    query_params: &mut Vec<(String, String)>,
) {
    let mut expand = |raw: &str| {
        let res = interpolate_with_map(raw, vars);
        for var in res.unresolved {
            if !unresolved.iter().any(|existing| existing == &var) {
                unresolved.push(var);
            }
        }
        res.rendered
    };

    match auth {
        EffectiveAuth::None => {}
        EffectiveAuth::Bearer { token, prefix } => {
            let expanded_token = expand(token);
            let expanded_prefix = expand(prefix);
            let value = if expanded_prefix.trim().is_empty() {
                expanded_token
            } else {
                format!("{} {}", expanded_prefix.trim(), expanded_token)
            };
            upsert_header(headers, "Authorization", value, true);
        }
        EffectiveAuth::Basic { username, password } => {
            let credentials = format!("{}:{}", expand(username), expand(password));
            let encoded = STANDARD.encode(credentials.as_bytes());
            upsert_header(headers, "Authorization", format!("Basic {encoded}"), true);
        }
        EffectiveAuth::ApiKey {
            key,
            value,
            location,
        } => {
            let k = expand(key);
            let v = expand(value);
            if !k.trim().is_empty() {
                match location {
                    ApiKeyLocation::Header => upsert_header(headers, k, v, true),
                    ApiKeyLocation::Query => query_params.push((k, v)),
                }
            }
        }
    }
}

fn upsert_header(
    headers: &mut Vec<PreparedHeader>,
    name: impl Into<String>,
    value: impl Into<String>,
    secret: bool,
) {
    let name = name.into();
    let value = value.into();
    if let Some(existing) = headers
        .iter_mut()
        .find(|h| h.name.eq_ignore_ascii_case(&name))
    {
        existing.name = name;
        existing.value = value;
        existing.secret = secret;
    } else {
        headers.push(PreparedHeader {
            name,
            value,
            secret,
        });
    }
}

fn upsert_header_if_missing(
    headers: &mut Vec<PreparedHeader>,
    name: impl Into<String>,
    value: impl Into<String>,
    secret: bool,
) {
    let name = name.into();
    if !headers.iter().any(|h| h.name.eq_ignore_ascii_case(&name)) {
        headers.push(PreparedHeader {
            name,
            value: value.into(),
            secret,
        });
    }
}
