use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    error::CoreError,
    model::{
        ApiKeyLocation, AuthConfig, EnvVariable, EnvironmentDoc, FolderDoc, HttpMethod,
        HttpRequestDoc, KeyValueRow, MultipartField, ProjectDoc, RawBodyFormat, RequestBody,
        generate_id,
    },
    response::ResponseHeader,
    response_meta::{
        ResponseExample, ResponseMetaField, extract_and_merge_response_meta,
        json_schema_to_meta_fields, merge_response_meta,
    },
};

/// A folder and its contained HTTP requests produced by importing an external specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFolder {
    pub doc: FolderDoc,
    pub requests: Vec<HttpRequestDoc>,
    pub folders: Vec<ImportedFolder>,
}

/// Complete in-memory project bundle parsed from an OpenAPI 3.0 document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedProjectBundle {
    pub project: ProjectDoc,
    pub environments: Vec<EnvironmentDoc>,
    pub folders: Vec<ImportedFolder>,
    pub root_requests: Vec<HttpRequestDoc>,
}

/// Parses an OpenAPI 3.0.x JSON specification (including Apifox exports) into a [`ImportedProjectBundle`].
pub fn parse_openapi_3_0(json_str: &str) -> Result<ImportedProjectBundle, CoreError> {
    let root: Value = serde_json::from_str(json_str).map_err(|err| CoreError::Validation {
        reason: format!("Invalid OpenAPI JSON: {err}"),
    })?;

    let version = root
        .get("openapi")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if !version.starts_with("3.0") {
        return Err(CoreError::Validation {
            reason: format!("Unsupported OpenAPI version '{version}'; expected 3.0.x"),
        });
    }

    let info = root.get("info");
    let title = info
        .and_then(|v| v.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported OpenAPI Project");
    let description = info
        .and_then(|v| v.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    let mut project = ProjectDoc::new(generate_id("proj"), title);
    project.description = description;

    let components = root.get("components");
    let schemas = components
        .and_then(|c| c.get("schemas"))
        .and_then(Value::as_object);
    let security_schemes = components
        .and_then(|c| c.get("securitySchemes"))
        .and_then(Value::as_object);
    let component_params = components
        .and_then(|c| c.get("parameters"))
        .and_then(Value::as_object);
    let component_bodies = components
        .and_then(|c| c.get("requestBodies"))
        .and_then(Value::as_object);
    let component_responses = components
        .and_then(|c| c.get("responses"))
        .and_then(Value::as_object);

    // Servers -> Project baseUrl + Environment profiles
    let servers = root
        .get("servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_servers = !servers.is_empty();
    if has_servers {
        project.base_url = "{{base_url}}".to_string();
    }

    // Security -> Project auth
    let root_security = root.get("security").and_then(Value::as_array);
    project.auth = root_security
        .and_then(|sec| resolve_security_requirement(sec, security_schemes))
        .unwrap_or(AuthConfig::None);

    let needs_api_token = matches!(
        &project.auth,
        AuthConfig::Bearer { token, .. } if token.contains("{{api_token}}")
    );

    let mut environments = Vec::new();
    if servers.is_empty() {
        let mut dev = EnvironmentDoc::new(generate_id("env"), "Development");
        if needs_api_token {
            dev.variables.push(
                EnvVariable::new("api_token", "")
                    .with_secret(true)
                    .with_description("Bearer token from OpenAPI security scheme"),
            );
        }
        environments.push(dev);
    } else {
        for (idx, srv) in servers.iter().enumerate() {
            let url = srv
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .trim_end_matches('/')
                .to_string();
            let desc = srv
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if idx == 0 {
                        "Development".to_string()
                    } else {
                        format!("Server {}", idx + 1)
                    }
                });
            let mut env = EnvironmentDoc::new(generate_id("env"), desc);
            env.variables.push(
                EnvVariable::new("base_url", url)
                    .with_description("Base URL imported from OpenAPI servers"),
            );
            if needs_api_token {
                env.variables.push(
                    EnvVariable::new("api_token", "")
                        .with_secret(true)
                        .with_description("Bearer token from OpenAPI security scheme"),
                );
            }
            environments.push(env);
        }
    }

    // Pre-seed folders from top-level `tags`
    let mut folders: Vec<ImportedFolder> = Vec::new();
    let mut tag_index: BTreeMap<String, usize> = BTreeMap::new();
    if let Some(tags) = root.get("tags").and_then(Value::as_array) {
        for tag in tags {
            let Some(name) = tag
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            if tag_index.contains_key(name) {
                continue;
            }
            let order = folders.len() as i32;
            let mut folder_doc = FolderDoc::new(generate_id("fld"), name);
            folder_doc.order = order;
            if let Some(desc) = tag.get("description").and_then(Value::as_str) {
                folder_doc.description = desc.trim().to_string();
            }
            tag_index.insert(name.to_string(), folders.len());
            folders.push(ImportedFolder {
                doc: folder_doc,
                requests: Vec::new(),
                folders: Vec::new(),
            });
        }
    }

    let mut root_requests: Vec<HttpRequestDoc> = Vec::new();
    let mut order_counter: i32 = 0;

    if let Some(paths) = root.get("paths").and_then(Value::as_object) {
        for (path_str, path_item) in paths {
            let Some(path_obj) = path_item.as_object() else {
                continue;
            };
            let path_params = path_obj
                .get("parameters")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);

            for (method_key, method) in [
                ("get", HttpMethod::Get),
                ("post", HttpMethod::Post),
                ("put", HttpMethod::Put),
                ("delete", HttpMethod::Delete),
                ("patch", HttpMethod::Patch),
                ("head", HttpMethod::Head),
                ("options", HttpMethod::Options),
            ] {
                let Some(op) = path_obj.get(method_key).and_then(Value::as_object) else {
                    continue;
                };

                let req_name = op
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        op.get("operationId")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                    })
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{} {}", method.as_str(), path_str));

                let url = if has_servers
                    && !path_str.starts_with("http://")
                    && !path_str.starts_with("https://")
                {
                    if path_str.starts_with('/') {
                        format!("{{{{base_url}}}}{path_str}")
                    } else {
                        format!("{{{{base_url}}}}/{path_str}")
                    }
                } else {
                    path_str.clone()
                };
                let mut req_doc = HttpRequestDoc::new(generate_id("req"), req_name, method, url);
                req_doc.order = order_counter;
                order_counter += 1;
                req_doc.description = op
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                // Merge path-level and operation-level parameters
                let op_params = op
                    .get("parameters")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let (query_params, header_params) =
                    collect_parameters(path_params, op_params, component_params);
                req_doc.request.params = query_params;
                req_doc.request.headers = header_params;

                // Auth override if operation defines `security`
                req_doc.request.auth = match op.get("security").and_then(Value::as_array) {
                    Some(sec) if sec.is_empty() => AuthConfig::None,
                    Some(sec) => match resolve_security_requirement(sec, security_schemes) {
                        Some(auth) if auth != project.auth => auth,
                        _ => AuthConfig::Inherit,
                    },
                    None => AuthConfig::Inherit,
                };

                // Request body
                if let Some(body_val) = op.get("requestBody") {
                    req_doc.request.body =
                        parse_request_body(body_val, component_bodies, schemas);
                }

                // Response examples & meta-info
                if let Some(responses_val) = op.get("responses") {
                    let (examples, meta) =
                        parse_openapi_responses(responses_val, component_responses, schemas);
                    req_doc.request.response_examples = examples;
                    req_doc.request.response_meta = meta;
                }

                // Place in first tag folder or root
                let tag_name = op
                    .get("tags")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());

                if let Some(tag) = tag_name {
                    let folder_ix = if let Some(&ix) = tag_index.get(tag) {
                        ix
                    } else {
                        let ix = folders.len();
                        tag_index.insert(tag.to_string(), ix);
                        let mut folder_doc = FolderDoc::new(generate_id("fld"), tag);
                        folder_doc.order = ix as i32;
                        folders.push(ImportedFolder {
                            doc: folder_doc,
                            requests: Vec::new(),
                            folders: Vec::new(),
                        });
                        ix
                    };
                    folders[folder_ix].requests.push(req_doc);
                } else {
                    root_requests.push(req_doc);
                }
            }
        }
    }

    Ok(ImportedProjectBundle {
        project,
        environments,
        folders,
        root_requests,
    })
}

/// Merges existing environment profiles into `bundle.environments` without overwriting
/// existing variables, then appends any new profiles/variables from the OpenAPI import.
pub fn merge_environments_into_bundle(
    bundle: &mut ImportedProjectBundle,
    existing_envs: &[EnvironmentDoc],
) {
    if existing_envs.is_empty() {
        return;
    }

    let imported_envs = std::mem::take(&mut bundle.environments);
    let mut merged: Vec<EnvironmentDoc> = existing_envs
        .iter()
        .map(|env| {
            let mut copy = env.clone();
            copy.id = generate_id("env");
            copy
        })
        .collect();

    for incoming in imported_envs {
        if let Some(target) = merged
            .iter_mut()
            .find(|e| e.name.eq_ignore_ascii_case(&incoming.name))
        {
            for var in incoming.variables {
                if !target
                    .variables
                    .iter()
                    .any(|v| v.key.trim() == var.key.trim())
                {
                    target.variables.push(var);
                }
            }
        } else {
            merged.push(incoming);
        }
    }

    bundle.environments = merged;
}

fn resolve_security_requirement(
    security: &[Value],
    schemes: Option<&serde_json::Map<String, Value>>,
) -> Option<AuthConfig> {
    let schemes = schemes?;
    for req_obj in security {
        let Some(map) = req_obj.as_object() else {
            continue;
        };
        for scheme_name in map.keys() {
            let Some(scheme) = schemes.get(scheme_name).and_then(Value::as_object) else {
                continue;
            };
            let scheme_type = scheme.get("type").and_then(Value::as_str).unwrap_or("");
            match scheme_type {
                "http" => {
                    let http_scheme = scheme
                        .get("scheme")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if http_scheme == "bearer" {
                        return Some(AuthConfig::Bearer {
                            token: "{{api_token}}".to_string(),
                            prefix: "Bearer".to_string(),
                        });
                    }
                    if http_scheme == "basic" {
                        return Some(AuthConfig::Basic {
                            username: "{{username}}".to_string(),
                            password: "{{password}}".to_string(),
                        });
                    }
                }
                "apiKey" => {
                    let key_name = scheme
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("X-API-Key")
                        .to_string();
                    let loc = match scheme.get("in").and_then(Value::as_str) {
                        Some("query") => ApiKeyLocation::Query,
                        _ => ApiKeyLocation::Header,
                    };
                    return Some(AuthConfig::ApiKey {
                        key: key_name,
                        value: "{{api_key}}".to_string(),
                        location: loc,
                    });
                }
                _ => {}
            }
        }
    }
    None
}

fn collect_parameters(
    path_params: &[Value],
    op_params: &[Value],
    component_params: Option<&serde_json::Map<String, Value>>,
) -> (Vec<KeyValueRow>, Vec<KeyValueRow>) {
    let mut merged: Vec<(String, String, KeyValueRow)> = Vec::new();

    for raw in path_params.iter().chain(op_params.iter()) {
        let param = resolve_ref(raw, "#/components/parameters/", component_params);
        let Some(obj) = param.as_object() else {
            continue;
        };
        let Some(name) = obj
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let location = obj.get("in").and_then(Value::as_str).unwrap_or("");
        if location != "query" && location != "header" {
            continue;
        }
        let value = extract_param_example(obj);
        let desc = obj
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let row = KeyValueRow::new(name, value).with_description(desc);

        if let Some(existing) = merged
            .iter_mut()
            .find(|(n, loc, _)| n == name && loc == location)
        {
            existing.2 = row;
        } else {
            merged.push((name.to_string(), location.to_string(), row));
        }
    }

    let mut query = Vec::new();
    let mut headers = Vec::new();
    for (_, loc, row) in merged {
        if loc == "query" {
            query.push(row);
        } else {
            headers.push(row);
        }
    }
    (query, headers)
}

fn extract_param_example(obj: &serde_json::Map<String, Value>) -> String {
    if let Some(v) = obj.get("example").and_then(scalar_to_string) {
        return v;
    }
    if let Some(examples) = obj.get("examples").and_then(Value::as_object)
        && let Some(first) = examples.values().next()
        && let Some(v) = first.get("value").and_then(scalar_to_string)
    {
        return v;
    }
    if let Some(schema) = obj.get("schema").and_then(Value::as_object) {
        if let Some(v) = schema.get("example").and_then(scalar_to_string) {
            return v;
        }
        if let Some(v) = schema.get("default").and_then(scalar_to_string) {
            return v;
        }
    }
    String::new()
}

fn scalar_to_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}

fn parse_request_body(
    raw_body: &Value,
    component_bodies: Option<&serde_json::Map<String, Value>>,
    schemas: Option<&serde_json::Map<String, Value>>,
) -> RequestBody {
    let body = resolve_ref(raw_body, "#/components/requestBodies/", component_bodies);
    let Some(content) = body.get("content").and_then(Value::as_object) else {
        return RequestBody::None;
    };

    if let Some((_, media)) = content.iter().find(|(ct, _)| ct.contains("json")) {
        let content_str = extract_json_body(media, schemas);
        return RequestBody::Json {
            content: content_str,
        };
    }

    if let Some(media) = content.get("application/x-www-form-urlencoded") {
        let fields = extract_kv_fields_from_media(media, schemas);
        return RequestBody::FormUrlEncoded { fields };
    }

    if let Some(media) = content.get("multipart/form-data") {
        let fields = extract_kv_fields_from_media(media, schemas)
            .into_iter()
            .map(|r| MultipartField::text(r.name, r.value))
            .collect();
        return RequestBody::Multipart { fields };
    }

    if let Some((ct, media)) = content.iter().next() {
        let raw_format = if ct.contains("xml") {
            RawBodyFormat::Xml
        } else if ct.contains("html") {
            RawBodyFormat::Html
        } else if ct.contains("javascript") {
            RawBodyFormat::Javascript
        } else if ct.contains("yaml") {
            RawBodyFormat::Yaml
        } else {
            RawBodyFormat::Text
        };
        let text = media
            .get("example")
            .and_then(scalar_to_string)
            .unwrap_or_default();
        return RequestBody::Raw {
            raw_format,
            content: text,
        };
    }

    RequestBody::None
}

fn extract_json_body(
    media: &Value,
    schemas: Option<&serde_json::Map<String, Value>>,
) -> String {
    if let Some(ex) = media.get("example") {
        if let Some(s) = ex.as_str() {
            return s.to_string();
        }
        return serde_json::to_string_pretty(ex).unwrap_or_else(|_| ex.to_string());
    }
    if let Some(examples) = media.get("examples").and_then(Value::as_object)
        && let Some(first) = examples.values().next()
        && let Some(val) = first.get("value")
    {
        if let Some(s) = val.as_str() {
            return s.to_string();
        }
        return serde_json::to_string_pretty(val).unwrap_or_else(|_| val.to_string());
    }
    if let Some(schema) = media.get("schema") {
        let synthesized = synthesize_value_from_schema(schema, schemas, 0);
        return serde_json::to_string_pretty(&synthesized).unwrap_or_else(|_| "{}".to_string());
    }
    "{}".to_string()
}

fn extract_kv_fields_from_media(
    media: &Value,
    schemas: Option<&serde_json::Map<String, Value>>,
) -> Vec<KeyValueRow> {
    if let Some(obj) = media.get("example").and_then(Value::as_object) {
        return obj
            .iter()
            .map(|(k, v)| KeyValueRow::new(k, scalar_to_string(v).unwrap_or_default()))
            .collect();
    }
    if let Some(schema) = media.get("schema") {
        let resolved = resolve_ref(schema, "#/components/schemas/", schemas);
        if let Some(props) = resolved.get("properties").and_then(Value::as_object) {
            return props
                .iter()
                .map(|(k, prop)| {
                    let desc = prop
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let val = prop
                        .get("example")
                        .or_else(|| prop.get("default"))
                        .and_then(scalar_to_string)
                        .unwrap_or_default();
                    KeyValueRow::new(k, val).with_description(desc)
                })
                .collect();
        }
    }
    Vec::new()
}

fn synthesize_value_from_schema(
    raw_schema: &Value,
    schemas: Option<&serde_json::Map<String, Value>>,
    depth: usize,
) -> Value {
    if depth > 6 {
        return Value::Object(serde_json::Map::new());
    }
    let schema = resolve_ref(raw_schema, "#/components/schemas/", schemas);
    if let Some(ex) = schema.get("example") {
        return ex.clone();
    }
    if let Some(def) = schema.get("default") {
        return def.clone();
    }

    let ty = schema.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "object" => {
            let mut map = serde_json::Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (k, prop) in props {
                    map.insert(k.clone(), synthesize_value_from_schema(prop, schemas, depth + 1));
                }
            }
            Value::Object(map)
        }
        "array" => {
            if let Some(items) = schema.get("items") {
                Value::Array(vec![synthesize_value_from_schema(items, schemas, depth + 1)])
            } else {
                Value::Array(Vec::new())
            }
        }
        "integer" => Value::Number(0.into()),
        "number" => serde_json::json!(0.0),
        "boolean" => Value::Bool(false),
        _ => Value::String(String::new()),
    }
}

fn resolve_ref<'a>(
    val: &'a Value,
    prefix: &str,
    components: Option<&'a serde_json::Map<String, Value>>,
) -> &'a Value {
    if let Some(ref_str) = val.get("$ref").and_then(Value::as_str)
        && let Some(name) = ref_str.strip_prefix(prefix)
        && let Some(target) = components.and_then(|c| c.get(name))
    {
        return target;
    }
    val
}

fn parse_openapi_responses(
    responses_val: &Value,
    component_responses: Option<&serde_json::Map<String, Value>>,
    schemas: Option<&serde_json::Map<String, Value>>,
) -> (Vec<ResponseExample>, Vec<ResponseMetaField>) {
    let mut examples = Vec::new();
    let mut meta = Vec::new();

    let Some(map) = responses_val.as_object() else {
        return (examples, meta);
    };

    for (status_key, raw_resp) in map {
        let resp = resolve_ref(raw_resp, "#/components/responses/", component_responses);
        let status_code = status_key.trim().parse::<u16>().unwrap_or(200);
        let reason = resp
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();

        let headers: Vec<ResponseHeader> = resp
            .get("headers")
            .and_then(Value::as_object)
            .map(|hdrs| {
                hdrs.iter()
                    .map(|(k, v)| {
                        let value = v
                            .get("example")
                            .or_else(|| v.get("schema").and_then(|s| s.get("example")))
                            .and_then(scalar_to_string)
                            .unwrap_or_default();
                        ResponseHeader {
                            name: k.clone(),
                            value,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let Some(content) = resp.get("content").and_then(Value::as_object) else {
            continue;
        };

        for (media_type, media_obj) in content {
            if let Some(schema) = media_obj.get("schema") {
                let schema_meta = json_schema_to_meta_fields(schema, schemas, 0);
                if !schema_meta.is_empty() {
                    merge_response_meta(&mut meta, schema_meta);
                }
            }

            if let Some(named_examples) = media_obj.get("examples").and_then(Value::as_object) {
                for (ex_key, ex_obj) in named_examples {
                    let Some(val) = ex_obj.get("value") else {
                        continue;
                    };
                    let body = format_openapi_example_body(val);
                    let ex_name = ex_obj
                        .get("summary")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(ex_key);
                    extract_and_merge_response_meta(&mut meta, &body, Some(media_type.as_str()));
                    examples.push(ResponseExample::new(
                        generate_id("ex"),
                        format!("{status_key} · {ex_name}"),
                        status_code,
                        reason.clone(),
                        Some(media_type.clone()),
                        headers.clone(),
                        body,
                    ));
                }
            } else if let Some(ex_val) = media_obj.get("example") {
                let body = format_openapi_example_body(ex_val);
                extract_and_merge_response_meta(&mut meta, &body, Some(media_type.as_str()));
                examples.push(ResponseExample::new(
                    generate_id("ex"),
                    status_key.clone(),
                    status_code,
                    reason.clone(),
                    Some(media_type.clone()),
                    headers.clone(),
                    body,
                ));
            } else if let Some(schema) = media_obj.get("schema") {
                let synthesized = synthesize_value_from_schema(schema, schemas, 0);
                let body = format_openapi_example_body(&synthesized);
                if !body.trim().is_empty() {
                    extract_and_merge_response_meta(&mut meta, &body, Some(media_type.as_str()));
                    examples.push(ResponseExample::new(
                        generate_id("ex"),
                        status_key.clone(),
                        status_code,
                        reason.clone(),
                        Some(media_type.clone()),
                        headers.clone(),
                        body,
                    ));
                }
            }
        }
    }

    (examples, meta)
}

fn format_openapi_example_body(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openapi_3_0_apifox_export_into_project_bundle() {
        let json = r#"{
          "openapi": "3.0.1",
          "info": { "title": "AppCore", "version": "1.0.0" },
          "tags": [{ "name": "门诊开单（老）" }, { "name": "医生交班" }],
          "servers": [{ "url": "http://localhost:30001/app-core", "description": "本地231" }],
          "security": [{ "bearer": [] }],
          "components": {
            "securitySchemes": {
              "bearer": { "type": "http", "scheme": "bearer" }
            }
          },
          "paths": {
            "/outpPresc/operate/saveOutpPresc": {
              "post": {
                "summary": "处方保存",
                "tags": ["门诊开单（老）"],
                "parameters": [
                  {
                    "name": "ek-b-prefer-dev-host",
                    "in": "header",
                    "example": "192.168.2.31",
                    "schema": { "type": "string", "default": "192.168.2.31" }
                  }
                ],
                "requestBody": {
                  "content": {
                    "application/json": {
                      "example": { "hospitalId": "711912746777575424", "packFlag": "1" }
                    }
                  }
                },
                "security": [{ "bearer": [] }]
              }
            },
            "/OutpPresc/query/listOpened": {
              "get": {
                "summary": "查询西药处方",
                "tags": ["门诊开单（老）"],
                "parameters": [
                  { "name": "visitId", "in": "query", "example": "1210558656748191744" }
                ]
              }
            },
            "/out/emer/hall/queryEmRescueRecord": {
              "post": {
                "summary": "查询急诊抢救留观记录",
                "tags": []
              }
            }
          }
        }"#;

        let bundle = parse_openapi_3_0(json).expect("valid OpenAPI 3.0");
        assert_eq!(bundle.project.name, "AppCore");
        assert_eq!(bundle.project.base_url, "{{base_url}}");
        assert!(matches!(bundle.project.auth, AuthConfig::Bearer { .. }));

        assert_eq!(bundle.environments.len(), 1);
        assert_eq!(bundle.environments[0].name, "本地231");
        assert_eq!(bundle.environments[0].variables[0].key, "base_url");
        assert_eq!(
            bundle.environments[0].variables[0].value,
            "http://localhost:30001/app-core"
        );
        assert_eq!(bundle.environments[0].variables[1].key, "api_token");

        assert_eq!(bundle.folders.len(), 2);
        assert_eq!(bundle.folders[0].doc.name, "门诊开单（老）");
        assert_eq!(bundle.folders[0].requests.len(), 2);
        assert_eq!(bundle.folders[1].doc.name, "医生交班");
        assert_eq!(bundle.folders[1].requests.len(), 0);

        assert_eq!(bundle.root_requests.len(), 1);
        assert_eq!(bundle.root_requests[0].name, "查询急诊抢救留观记录");

        let save_req = bundle.folders[0]
            .requests
            .iter()
            .find(|r| r.name == "处方保存")
            .unwrap();
        assert_eq!(save_req.request.method, HttpMethod::Post);
        assert_eq!(
            save_req.request.url,
            "{{base_url}}/outpPresc/operate/saveOutpPresc"
        );
        assert_eq!(save_req.request.headers[0].name, "ek-b-prefer-dev-host");
        assert_eq!(save_req.request.headers[0].value, "192.168.2.31");
        assert!(matches!(save_req.request.body, RequestBody::Json { .. }));
    }

    #[test]
    fn rejects_non_openapi_3_0_documents() {
        let err = parse_openapi_3_0(r#"{"swagger":"2.0","info":{"title":"Legacy"}}"#).unwrap_err();
        assert!(err.to_string().contains("3.0"));
    }

    #[test]
    fn merges_existing_profiles_into_new_project_bundle_without_overwriting() {
        let json = r#"{
          "openapi": "3.0.1",
          "info": { "title": "AppCore", "version": "1.0.0" },
          "servers": [{ "url": "http://localhost:30001/app-core", "description": "Development" }],
          "paths": {}
        }"#;
        let mut bundle = parse_openapi_3_0(json).unwrap();

        let mut existing = EnvironmentDoc::new("env-old", "Development");
        existing
            .variables
            .push(EnvVariable::new("base_url", "http://192.168.1.231:4433"));
        existing
            .variables
            .push(EnvVariable::new("api_token", "keep-secret-token"));

        merge_environments_into_bundle(&mut bundle, &[existing]);

        assert_eq!(bundle.environments.len(), 1);
        assert_eq!(bundle.environments[0].name, "Development");
        assert_eq!(
            bundle.environments[0].variables[0].value,
            "http://192.168.1.231:4433"
        );
        assert_eq!(
            bundle.environments[0].variables[1].value,
            "keep-secret-token"
        );
    }

    #[test]
    fn imports_openapi_response_examples_and_schema_meta() {
        let json = r#"{
          "openapi": "3.0.1",
          "info": { "title": "MetaDemo", "version": "1.0.0" },
          "paths": {
            "/users": {
              "get": {
                "summary": "List Users",
                "responses": {
                  "200": {
                    "description": "OK",
                    "content": {
                      "application/json": {
                        "schema": {
                          "type": "object",
                          "properties": {
                            "id": { "type": "integer", "description": "User ID", "example": 42 },
                            "name": { "type": "string", "description": "Full name" }
                          }
                        },
                        "example": { "id": 42, "name": "Ada" }
                      }
                    }
                  }
                }
              }
            }
          }
        }"#;
        let bundle = parse_openapi_3_0(json).unwrap();
        let req = &bundle.root_requests[0];
        assert_eq!(req.request.response_examples.len(), 1);
        assert_eq!(req.request.response_examples[0].status_code, 200);
        assert!(req.request.response_examples[0].body.contains("Ada"));
        assert_eq!(req.request.response_meta.len(), 2);
        let id_field = req
            .request
            .response_meta
            .iter()
            .find(|f| f.name == "id")
            .unwrap();
        assert_eq!(id_field.description, "User ID");
        assert_eq!(id_field.example, "42");
    }
}
