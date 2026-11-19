use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    error::CoreError,
    model::{
        ApiKeyLocation, AuthConfig, EnvVariable, EnvironmentDoc, FolderDoc, FolderNode,
        HttpMethod, HttpRequestDoc, KeyValueRow, MultipartField, MultipartFieldType, ProjectDoc,
        ProjectNode, RawBodyFormat, RequestBody, RequestNode, generate_id,
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

/// Exports a [`ProjectNode`] as an OpenAPI 3.0.1 JSON document string.
pub fn export_openapi_3_0(project: &ProjectNode) -> Result<String, CoreError> {
    let mut info = serde_json::Map::new();
    info.insert("title".to_string(), Value::String(project.doc.name.clone()));
    info.insert("version".to_string(), Value::String("1.0.0".to_string()));
    if !project.doc.description.trim().is_empty() {
        info.insert(
            "description".to_string(),
            Value::String(project.doc.description.clone()),
        );
    }

    let mut servers = Vec::new();
    for env in &project.environments {
        if let Some(var) = env
            .doc
            .variables
            .iter()
            .find(|v| v.enabled && (v.key == "base_url" || v.key == "baseUrl") && !v.value.trim().is_empty())
        {
            servers.push(serde_json::json!({
                "url": var.value,
                "description": env.doc.name,
            }));
        }
    }
    if servers.is_empty()
        && !project.doc.base_url.trim().is_empty()
        && !project.doc.base_url.contains("{{")
    {
        servers.push(serde_json::json!({ "url": project.doc.base_url }));
    }

    let mut security_schemes = serde_json::Map::new();
    let mut tags = Vec::new();
    let mut paths: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();

    if let Some(scheme_name) = register_security_scheme(&project.doc.auth, &mut security_schemes) {
        let _ = scheme_name;
    }

    for req in &project.requests {
        export_request_into_paths(
            req,
            None,
            &project.doc.base_url,
            &mut security_schemes,
            &mut paths,
        );
    }

    for folder in &project.folders {
        export_folder_into_paths(
            folder,
            None,
            &project.doc.base_url,
            &mut tags,
            &mut security_schemes,
            &mut paths,
        );
    }

    let mut root = serde_json::Map::new();
    root.insert("openapi".to_string(), Value::String("3.0.1".to_string()));
    root.insert("info".to_string(), Value::Object(info));
    if !servers.is_empty() {
        root.insert("servers".to_string(), Value::Array(servers));
    }
    if !tags.is_empty() {
        root.insert("tags".to_string(), Value::Array(tags));
    }
    root.insert(
        "paths".to_string(),
        Value::Object(
            paths
                .into_iter()
                .map(|(p, ops)| (p, Value::Object(ops)))
                .collect(),
        ),
    );
    if !security_schemes.is_empty() {
        root.insert(
            "components".to_string(),
            serde_json::json!({ "securitySchemes": security_schemes }),
        );
    }
    if let Some(sec_name) = register_security_scheme(&project.doc.auth, &mut serde_json::Map::new()) {
        root.insert("security".to_string(), serde_json::json!([{ sec_name: [] }]));
    }

    serde_json::to_string_pretty(&Value::Object(root)).map_err(|err| CoreError::Validation {
        reason: format!("Failed serializing OpenAPI JSON: {err}"),
    })
}

fn export_folder_into_paths(
    folder: &FolderNode,
    parent_tag: Option<&str>,
    project_base_url: &str,
    tags: &mut Vec<Value>,
    security_schemes: &mut serde_json::Map<String, Value>,
    paths: &mut BTreeMap<String, serde_json::Map<String, Value>>,
) {
    let tag_name = match parent_tag {
        Some(parent) => format!("{parent} / {}", folder.doc.name),
        None => folder.doc.name.clone(),
    };
    let mut tag_obj = serde_json::Map::new();
    tag_obj.insert("name".to_string(), Value::String(tag_name.clone()));
    if !folder.doc.description.trim().is_empty() {
        tag_obj.insert(
            "description".to_string(),
            Value::String(folder.doc.description.clone()),
        );
    }
    tags.push(Value::Object(tag_obj));

    for req in &folder.requests {
        export_request_into_paths(
            req,
            Some(&tag_name),
            project_base_url,
            security_schemes,
            paths,
        );
    }
    for sub in &folder.folders {
        export_folder_into_paths(
            sub,
            Some(&tag_name),
            project_base_url,
            tags,
            security_schemes,
            paths,
        );
    }
}

fn export_request_into_paths(
    req: &RequestNode,
    tag: Option<&str>,
    project_base_url: &str,
    security_schemes: &mut serde_json::Map<String, Value>,
    paths: &mut BTreeMap<String, serde_json::Map<String, Value>>,
) {
    let path_str = normalize_export_path(&req.doc.request.url, project_base_url);
    let method_key = match req.doc.request.method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        HttpMethod::Head => "head",
        HttpMethod::Options => "options",
    };

    let mut op = serde_json::Map::new();
    op.insert("summary".to_string(), Value::String(req.doc.name.clone()));
    if !req.doc.description.trim().is_empty() {
        op.insert(
            "description".to_string(),
            Value::String(req.doc.description.clone()),
        );
    }
    if let Some(tag_name) = tag {
        op.insert(
            "tags".to_string(),
            Value::Array(vec![Value::String(tag_name.to_string())]),
        );
    }

    let mut params = Vec::new();
    for p in &req.doc.request.params {
        if p.name.trim().is_empty() {
            continue;
        }
        let mut p_obj = serde_json::Map::new();
        p_obj.insert("name".to_string(), Value::String(p.name.clone()));
        p_obj.insert("in".to_string(), Value::String("query".to_string()));
        if !p.description.trim().is_empty() {
            p_obj.insert(
                "description".to_string(),
                Value::String(p.description.clone()),
            );
        }
        let mut schema = serde_json::Map::new();
        schema.insert("type".to_string(), Value::String("string".to_string()));
        if !p.value.is_empty() {
            schema.insert("example".to_string(), Value::String(p.value.clone()));
        }
        p_obj.insert("schema".to_string(), Value::Object(schema));
        params.push(Value::Object(p_obj));
    }
    for h in &req.doc.request.headers {
        if h.name.trim().is_empty() {
            continue;
        }
        let mut h_obj = serde_json::Map::new();
        h_obj.insert("name".to_string(), Value::String(h.name.clone()));
        h_obj.insert("in".to_string(), Value::String("header".to_string()));
        if !h.description.trim().is_empty() {
            h_obj.insert(
                "description".to_string(),
                Value::String(h.description.clone()),
            );
        }
        let mut schema = serde_json::Map::new();
        schema.insert("type".to_string(), Value::String("string".to_string()));
        if !h.value.is_empty() {
            schema.insert("example".to_string(), Value::String(h.value.clone()));
        }
        h_obj.insert("schema".to_string(), Value::Object(schema));
        params.push(Value::Object(h_obj));
    }
    if !params.is_empty() {
        op.insert("parameters".to_string(), Value::Array(params));
    }

    if let Some(req_body) = export_request_body(&req.doc.request.body) {
        op.insert("requestBody".to_string(), req_body);
    }

    match &req.doc.request.auth {
        AuthConfig::Inherit => {}
        AuthConfig::None => {
            op.insert("security".to_string(), Value::Array(Vec::new()));
        }
        other => {
            if let Some(sec_name) = register_security_scheme(other, security_schemes) {
                op.insert("security".to_string(), serde_json::json!([{ sec_name: [] }]));
            }
        }
    }

    op.insert(
        "responses".to_string(),
        export_responses(
            &req.doc.request.response_examples,
            &req.doc.request.response_meta,
        ),
    );

    paths
        .entry(path_str)
        .or_default()
        .insert(method_key.to_string(), Value::Object(op));
}

fn normalize_export_path(raw_url: &str, project_base_url: &str) -> String {
    let trimmed = raw_url.trim();
    let without_query = trimmed.split('?').next().unwrap_or(trimmed);
    let path = without_query
        .strip_prefix("{{base_url}}")
        .or_else(|| without_query.strip_prefix("{{baseUrl}}"))
        .or_else(|| {
            let base = project_base_url.trim().trim_end_matches('/');
            if !base.is_empty() {
                without_query.strip_prefix(base)
            } else {
                None
            }
        })
        .unwrap_or(without_query);

    let path_str = if let Ok(parsed) = url::Url::parse(path) {
        parsed.path().to_string()
    } else {
        path.to_string()
    };

    let path = path_str.trim();
    if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn export_request_body(body: &RequestBody) -> Option<Value> {
    match body {
        RequestBody::None => None,
        RequestBody::Json { content } => {
            let example_val = serde_json::from_str::<Value>(content)
                .unwrap_or_else(|_| Value::String(content.clone()));
            Some(serde_json::json!({
                "content": {
                    "application/json": {
                        "schema": { "type": "object" },
                        "example": example_val
                    }
                }
            }))
        }
        RequestBody::FormUrlEncoded { fields } => {
            let props = kv_rows_to_schema_props(fields);
            Some(serde_json::json!({
                "content": {
                    "application/x-www-form-urlencoded": {
                        "schema": {
                            "type": "object",
                            "properties": props
                        }
                    }
                }
            }))
        }
        RequestBody::Multipart { fields } => {
            let mut props = serde_json::Map::new();
            for f in fields {
                if f.name.trim().is_empty() {
                    continue;
                }
                let mut prop = serde_json::Map::new();
                prop.insert("type".to_string(), Value::String("string".to_string()));
                if f.field_type == MultipartFieldType::File {
                    prop.insert("format".to_string(), Value::String("binary".to_string()));
                } else if !f.value.is_empty() {
                    prop.insert("example".to_string(), Value::String(f.value.clone()));
                }
                if !f.description.trim().is_empty() {
                    prop.insert(
                        "description".to_string(),
                        Value::String(f.description.clone()),
                    );
                }
                props.insert(f.name.clone(), Value::Object(prop));
            }
            Some(serde_json::json!({
                "content": {
                    "multipart/form-data": {
                        "schema": {
                            "type": "object",
                            "properties": props
                        }
                    }
                }
            }))
        }
        RequestBody::Raw {
            raw_format,
            content,
        } => {
            let media_type = match raw_format {
                RawBodyFormat::Xml => "application/xml",
                RawBodyFormat::Yaml => "application/yaml",
                RawBodyFormat::Html => "text/html",
                RawBodyFormat::Javascript => "application/javascript",
                RawBodyFormat::Text => "text/plain",
            };
            Some(serde_json::json!({
                "content": {
                    media_type: {
                        "schema": { "type": "string" },
                        "example": content
                    }
                }
            }))
        }
        RequestBody::Binary { .. } => Some(serde_json::json!({
            "content": {
                "application/octet-stream": {
                    "schema": { "type": "string", "format": "binary" }
                }
            }
        })),
    }
}

fn kv_rows_to_schema_props(rows: &[KeyValueRow]) -> serde_json::Map<String, Value> {
    let mut props = serde_json::Map::new();
    for r in rows {
        if r.name.trim().is_empty() {
            continue;
        }
        let mut prop = serde_json::Map::new();
        prop.insert("type".to_string(), Value::String("string".to_string()));
        if !r.value.is_empty() {
            prop.insert("example".to_string(), Value::String(r.value.clone()));
        }
        if !r.description.trim().is_empty() {
            prop.insert(
                "description".to_string(),
                Value::String(r.description.clone()),
            );
        }
        props.insert(r.name.clone(), Value::Object(prop));
    }
    props
}

fn export_responses(
    examples: &[ResponseExample],
    meta: &[ResponseMetaField],
) -> Value {
    let mut responses = serde_json::Map::new();
    let meta_schema = if meta.is_empty() {
        None
    } else {
        Some(meta_fields_to_json_schema(meta))
    };

    if examples.is_empty() {
        let mut resp = serde_json::Map::new();
        resp.insert("description".to_string(), Value::String("OK".to_string()));
        if let Some(schema) = meta_schema {
            resp.insert(
                "content".to_string(),
                serde_json::json!({ "application/json": { "schema": schema } }),
            );
        }
        responses.insert("200".to_string(), Value::Object(resp));
        return Value::Object(responses);
    }

    let mut grouped: BTreeMap<u16, Vec<&ResponseExample>> = BTreeMap::new();
    for ex in examples {
        let code = if ex.status_code == 0 { 200 } else { ex.status_code };
        grouped.entry(code).or_default().push(ex);
    }

    for (code, group) in grouped {
        let first = group[0];
        let reason = if first.reason.trim().is_empty() {
            "Response"
        } else {
            first.reason.trim()
        };
        let mut resp = serde_json::Map::new();
        resp.insert("description".to_string(), Value::String(reason.to_string()));

        if !first.headers.is_empty() {
            let mut hdrs = serde_json::Map::new();
            for h in &first.headers {
                if !h.name.trim().is_empty() {
                    hdrs.insert(
                        h.name.clone(),
                        serde_json::json!({
                            "schema": { "type": "string", "example": h.value }
                        }),
                    );
                }
            }
            if !hdrs.is_empty() {
                resp.insert("headers".to_string(), Value::Object(hdrs));
            }
        }

        let mut content = serde_json::Map::new();
        let media_type = first
            .content_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("application/json")
            .split(';')
            .next()
            .unwrap_or("application/json")
            .trim()
            .to_string();

        let mut media_obj = serde_json::Map::new();
        if let Some(schema) = &meta_schema {
            media_obj.insert("schema".to_string(), schema.clone());
        }
        if group.len() == 1 {
            let parsed_body = serde_json::from_str::<Value>(&first.body)
                .unwrap_or_else(|_| Value::String(first.body.clone()));
            media_obj.insert("example".to_string(), parsed_body);
        } else {
            let mut named = serde_json::Map::new();
            for ex in group {
                let parsed_body = serde_json::from_str::<Value>(&ex.body)
                    .unwrap_or_else(|_| Value::String(ex.body.clone()));
                named.insert(
                    ex.name.clone(),
                    serde_json::json!({ "summary": ex.name, "value": parsed_body }),
                );
            }
            media_obj.insert("examples".to_string(), Value::Object(named));
        }

        content.insert(media_type, Value::Object(media_obj));
        resp.insert("content".to_string(), Value::Object(content));
        responses.insert(code.to_string(), Value::Object(resp));
    }

    Value::Object(responses)
}

fn meta_fields_to_json_schema(fields: &[ResponseMetaField]) -> Value {
    if fields.len() == 1 && fields[0].name == "[]" {
        let items = meta_fields_to_json_schema(&fields[0].children);
        let mut arr = serde_json::Map::new();
        arr.insert("type".to_string(), Value::String("array".to_string()));
        arr.insert("items".to_string(), items);
        if !fields[0].description.trim().is_empty() {
            arr.insert(
                "description".to_string(),
                Value::String(fields[0].description.clone()),
            );
        }
        return Value::Object(arr);
    }

    let mut props = serde_json::Map::new();
    for f in fields {
        let mut prop = serde_json::Map::new();
        let ty = f
            .field_type
            .split(" | ")
            .next()
            .unwrap_or("string")
            .trim();
        let json_ty = match ty {
            "integer" | "number" | "boolean" | "object" | "array" => ty,
            _ => "string",
        };
        prop.insert("type".to_string(), Value::String(json_ty.to_string()));
        if !f.description.trim().is_empty() {
            prop.insert(
                "description".to_string(),
                Value::String(f.description.clone()),
            );
        }
        if !f.example.trim().is_empty() {
            prop.insert("example".to_string(), Value::String(f.example.clone()));
        }
        if !f.children.is_empty() {
            if json_ty == "array" {
                prop.insert("items".to_string(), meta_fields_to_json_schema(&f.children));
            } else {
                let nested = meta_fields_to_json_schema(&f.children);
                if let Some(nested_props) = nested.get("properties") {
                    prop.insert("properties".to_string(), nested_props.clone());
                }
            }
        }
        props.insert(f.name.clone(), Value::Object(prop));
    }

    serde_json::json!({
        "type": "object",
        "properties": props
    })
}

fn register_security_scheme(
    auth: &AuthConfig,
    schemes: &mut serde_json::Map<String, Value>,
) -> Option<&'static str> {
    match auth {
        AuthConfig::Bearer { .. } => {
            schemes.insert(
                "bearerAuth".to_string(),
                serde_json::json!({ "type": "http", "scheme": "bearer" }),
            );
            Some("bearerAuth")
        }
        AuthConfig::Basic { .. } => {
            schemes.insert(
                "basicAuth".to_string(),
                serde_json::json!({ "type": "http", "scheme": "basic" }),
            );
            Some("basicAuth")
        }
        AuthConfig::ApiKey { key, location, .. } => {
            let in_loc = match location {
                ApiKeyLocation::Query => "query",
                ApiKeyLocation::Header => "header",
            };
            schemes.insert(
                "apiKeyAuth".to_string(),
                serde_json::json!({
                    "type": "apiKey",
                    "in": in_loc,
                    "name": if key.trim().is_empty() { "X-API-Key" } else { key.trim() }
                }),
            );
            Some("apiKeyAuth")
        }
        AuthConfig::Inherit | AuthConfig::None => None,
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

    #[test]
    fn exports_project_to_openapi_3_0_and_roundtrips() {
        let mut proj_doc = ProjectDoc::new("proj-exp", "Exported API");
        proj_doc.base_url = "{{base_url}}".to_string();
        proj_doc.auth = AuthConfig::Bearer {
            token: "{{token}}".to_string(),
            prefix: "Bearer".to_string(),
        };

        let mut env_doc = EnvironmentDoc::new("env-1", "Prod");
        env_doc
            .variables
            .push(EnvVariable::new("base_url", "https://api.example.com"));

        let mut req_doc = HttpRequestDoc::new(
            "req-1",
            "Get User",
            HttpMethod::Get,
            "{{base_url}}/users",
        );
        req_doc
            .request
            .params
            .push(KeyValueRow::new("active", "true"));
        req_doc.request.response_examples.push(ResponseExample::new(
            "ex-1",
            "200",
            200,
            "OK",
            Some("application/json".to_string()),
            Vec::new(),
            r#"{"id":1,"name":"Ada"}"#,
        ));
        req_doc.request.response_meta.push(
            ResponseMetaField::new("name", "string").with_description("User full name"),
        );

        let project = ProjectNode {
            root_dir: std::path::PathBuf::from("/tmp/proj-exp"),
            manifest_path: std::path::PathBuf::from("/tmp/proj-exp/project.yaml"),
            doc: proj_doc,
            environments: vec![crate::model::EnvironmentNode {
                file_path: std::path::PathBuf::from("/tmp/proj-exp/environments/prod.env.yaml"),
                doc: env_doc,
            }],
            folders: Vec::new(),
            requests: vec![RequestNode {
                file_path: std::path::PathBuf::from("/tmp/proj-exp/get-user.request.yaml"),
                doc: req_doc,
            }],
        };

        let exported_json = export_openapi_3_0(&project).expect("export openapi");
        let reimported = parse_openapi_3_0(&exported_json).expect("reimport exported openapi");
        assert_eq!(reimported.project.name, "Exported API");
        assert_eq!(reimported.environments[0].name, "Prod");
        assert_eq!(reimported.root_requests.len(), 1);
        let re_req = &reimported.root_requests[0];
        assert_eq!(re_req.name, "Get User");
        assert_eq!(re_req.request.params.len(), 1);
        assert_eq!(re_req.request.response_examples.len(), 1);
        assert!(
            re_req
                .request
                .response_meta
                .iter()
                .any(|f| f.name == "name" && f.description == "User full name")
        );
    }
}
