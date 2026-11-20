use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    let path_pairs: Vec<(String, String)> = doc
        .request
        .path_params
        .iter()
        .filter(|p| p.enabled && !p.name.trim().is_empty())
        .map(|p| (p.name.trim().to_string(), expand(&p.value, &mut unresolved)))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    let raw_url = apply_path_params(&expand(&doc.request.url, &mut unresolved), &path_pairs);
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

/// Extracts `key=value` query parameters from `raw_url` when a `?query` with `=` is present.
///
/// Returns `Some((url_without_query, params))` if at least one non-empty parameter was parsed.
pub fn extract_url_query_params(raw_url: &str) -> Option<(String, Vec<(String, String)>)> {
    let q_ix = raw_url.find('?')?;
    let base = &raw_url[..q_ix];
    let after_q = &raw_url[q_ix + 1..];
    let (query_str, fragment) = match after_q.find('#') {
        Some(f_ix) => (&after_q[..f_ix], &after_q[f_ix..]),
        None => (after_q, ""),
    };
    if !query_str.contains('=') {
        return None;
    }

    let params: Vec<(String, String)> = url::form_urlencoded::parse(query_str.as_bytes())
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if params.is_empty() {
        return None;
    }

    Some((format!("{base}{fragment}"), params))
}

fn split_url_path_slice(raw_url: &str) -> (&str, &str, &str) {
    let end = raw_url.find(['?', '#']).unwrap_or(raw_url.len());
    let before_query = &raw_url[..end];
    let suffix = &raw_url[end..];

    if let Some(scheme_ix) = before_query.find("://") {
        let after_scheme = &before_query[scheme_ix + 3..];
        if let Some(slash_ix) = after_scheme.find('/') {
            let path_start = scheme_ix + 3 + slash_ix;
            return (&before_query[..path_start], &before_query[path_start..], suffix);
        }
        return (before_query, "", suffix);
    }

    ("", before_query, suffix)
}

/// Extracts path variable names (`:name` or `{name}`, ignoring `{{env_var}}`) from a URL.
pub fn extract_path_param_names(raw_url: &str) -> Vec<String> {
    let (_, path, _) = split_url_path_slice(raw_url);
    let mut out = Vec::new();
    let mut push_unique = |name: &str| {
        let trimmed = name.trim();
        if !trimmed.is_empty() && !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    };

    for segment in path.split('/') {
        let mut rest = segment;
        while let Some(open) = rest.find('{') {
            if rest[open..].starts_with("{{") {
                if let Some(close) = rest[open + 2..].find("}}") {
                    rest = &rest[open + 2 + close + 2..];
                    continue;
                }
                break;
            }
            if let Some(close) = rest[open + 1..].find('}') {
                let name = &rest[open + 1..open + 1 + close];
                if !name.contains('{') {
                    push_unique(name);
                }
                rest = &rest[open + 1 + close + 1..];
            } else {
                break;
            }
        }
        if let Some(colon_name) = segment.strip_prefix(':') {
            let valid = !colon_name.is_empty()
                && !colon_name.starts_with('{')
                && colon_name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if valid {
                push_unique(colon_name);
            }
        }
    }

    out
}

/// Substitutes `{name}` and `:name` path variables in `raw_url` with their configured values.
pub fn apply_path_params(raw_url: &str, params: &[(String, String)]) -> String {
    if params.is_empty() {
        return raw_url.to_string();
    }
    let (prefix, path, suffix) = split_url_path_slice(raw_url);
    if path.is_empty() {
        return raw_url.to_string();
    }

    let mut replaced_segments = Vec::new();
    for segment in path.split('/') {
        if let Some(colon_name) = segment.strip_prefix(':')
            && let Some((_, val)) = params.iter().find(|(k, _)| k == colon_name)
        {
            replaced_segments.push(val.clone());
            continue;
        }

        let mut seg_out = String::with_capacity(segment.len());
        let mut rest = segment;
        while let Some(open) = rest.find('{') {
            if rest[open..].starts_with("{{") {
                if let Some(close) = rest[open + 2..].find("}}") {
                    let end = open + 2 + close + 2;
                    seg_out.push_str(&rest[..end]);
                    rest = &rest[end..];
                    continue;
                }
                break;
            }
            seg_out.push_str(&rest[..open]);
            if let Some(close) = rest[open + 1..].find('}') {
                let name = rest[open + 1..open + 1 + close].trim();
                if let Some((_, val)) = params.iter().find(|(k, _)| k == name) {
                    seg_out.push_str(val);
                } else {
                    seg_out.push_str(&rest[open..open + 1 + close + 1]);
                }
                rest = &rest[open + 1 + close + 1..];
            } else {
                seg_out.push_str(&rest[open..]);
                rest = "";
                break;
            }
        }
        seg_out.push_str(rest);
        replaced_segments.push(seg_out);
    }

    format!("{prefix}{}{suffix}", replaced_segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_applies_path_variables() {
        assert_eq!(
            extract_path_param_names("https://localhost:8080/users/:userId/orders/{order_id}?a=1"),
            vec!["userId".to_string(), "order_id".to_string()]
        );
        assert_eq!(
            extract_path_param_names("{{base_url}}/items/{{item_id}}/:id"),
            vec!["id".to_string()]
        );
        assert_eq!(
            apply_path_params(
                "https://localhost:8080/users/:userId/orders/{order_id}?a=1",
                &[
                    ("userId".to_string(), "42".to_string()),
                    ("order_id".to_string(), "ord-9".to_string()),
                ],
            ),
            "https://localhost:8080/users/42/orders/ord-9?a=1"
        );
    }

    #[test]
    fn extracts_query_params_from_pasted_url() {
        assert_eq!(
            extract_url_query_params("/index/visitId?visitId=1167904573952622592"),
            Some((
                "/index/visitId".to_string(),
                vec![("visitId".to_string(), "1167904573952622592".to_string())],
            ))
        );
        assert_eq!(
            extract_url_query_params("/users?page=2&q={{query}}#top"),
            Some((
                "/users#top".to_string(),
                vec![
                    ("page".to_string(), "2".to_string()),
                    ("q".to_string(), "{{query}}".to_string()),
                ],
            ))
        );
        assert_eq!(extract_url_query_params("/users?"), None);
        assert_eq!(extract_url_query_params("/users?pending"), None);
    }
}
