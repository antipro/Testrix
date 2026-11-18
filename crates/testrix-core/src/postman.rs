use std::io::{Cursor, Read};

use serde_json::Value;
use zip::ZipArchive;

use crate::{
    error::CoreError,
    model::{
        ApiKeyLocation, AuthConfig, EnvVariable, EnvironmentDoc, FolderDoc, HttpMethod,
        HttpRequestDoc, KeyValueRow, MultipartField, ProjectDoc, RawBodyFormat, RequestBody,
        generate_id,
    },
    openapi::{ImportedFolder, ImportedProjectBundle},
};

/// Parses a Postman Collection v2.0 / v2.1 JSON string into an [`ImportedProjectBundle`].
pub fn parse_postman_collection(json_str: &str) -> Result<ImportedProjectBundle, CoreError> {
    let root: Value = serde_json::from_str(json_str).map_err(|err| CoreError::Validation {
        reason: format!("Invalid Postman collection JSON: {err}"),
    })?;
    parse_postman_collection_value(&root)
}

/// Parses a Postman Environment JSON string into an [`EnvironmentDoc`].
pub fn parse_postman_environment(json_str: &str) -> Result<EnvironmentDoc, CoreError> {
    let root: Value = serde_json::from_str(json_str).map_err(|err| CoreError::Validation {
        reason: format!("Invalid Postman environment JSON: {err}"),
    })?;
    parse_postman_environment_value(&root)
}

/// Parses a `.zip` archive containing one or more Postman Collection JSON files and optional
/// Postman Environment JSON files into [`ImportedProjectBundle`]s.
pub fn parse_postman_zip(bytes: &[u8]) -> Result<Vec<ImportedProjectBundle>, CoreError> {
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|err| CoreError::Validation {
        reason: format!("Invalid ZIP archive: {err}"),
    })?;

    let mut collections: Vec<ImportedProjectBundle> = Vec::new();
    let mut environments: Vec<EnvironmentDoc> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|err| CoreError::Validation {
            reason: format!("Failed reading ZIP entry #{i}: {err}"),
        })?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if name.starts_with("__MACOSX/")
            || name
                .rsplit('/')
                .next()
                .is_some_and(|base| base.starts_with('.'))
        {
            continue;
        }
        if !name.to_ascii_lowercase().ends_with(".json") {
            continue;
        }

        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .map_err(|err| CoreError::Validation {
                reason: format!("Failed reading '{name}' as UTF-8 JSON: {err}"),
            })?;

        let root: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if is_postman_collection(&root) {
            collections.push(parse_postman_collection_value(&root)?);
        } else if is_postman_environment(&root) {
            environments.push(parse_postman_environment_value(&root)?);
        }
    }

    if collections.is_empty() {
        return Err(CoreError::Validation {
            reason: "No Postman collection JSON found inside the ZIP archive.".to_string(),
        });
    }

    if !environments.is_empty() {
        for bundle in &mut collections {
            for env in &environments {
                if let Some(existing) = bundle
                    .environments
                    .iter_mut()
                    .find(|e| e.name.eq_ignore_ascii_case(&env.name))
                {
                    for var in &env.variables {
                        if !existing
                            .variables
                            .iter()
                            .any(|v| v.key.trim() == var.key.trim())
                        {
                            existing.variables.push(var.clone());
                        }
                    }
                } else {
                    let mut cloned = env.clone();
                    cloned.id = generate_id("env");
                    bundle.environments.push(cloned);
                }
            }
        }
    }

    Ok(collections)
}

fn is_postman_collection(root: &Value) -> bool {
    let has_items = root.get("item").and_then(Value::as_array).is_some();
    let is_schema = root
        .get("info")
        .and_then(|v| v.get("schema"))
        .and_then(Value::as_str)
        .is_some_and(|s| s.contains("getpostman.com") && s.contains("collection"));
    has_items || is_schema
}

fn is_postman_environment(root: &Value) -> bool {
    let has_values = root.get("values").and_then(Value::as_array).is_some();
    let is_env_scope = root
        .get("_postman_variable_scope")
        .and_then(Value::as_str)
        .is_some_and(|s| s == "environment");
    has_values && (is_env_scope || root.get("item").is_none())
}

fn parse_postman_environment_value(root: &Value) -> Result<EnvironmentDoc, CoreError> {
    let name = root
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported Postman Env");

    let mut env = EnvironmentDoc::new(generate_id("env"), name);
    if let Some(values) = root.get("values").and_then(Value::as_array) {
        for item in values {
            let key = item
                .get("key")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if key.is_empty() {
                continue;
            }
            let value = value_to_string(item.get("value"));
            let enabled = item.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let secret = item
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.eq_ignore_ascii_case("secret"));
            env.variables.push(EnvVariable {
                key: key.to_string(),
                value,
                enabled,
                secret,
                description: extract_description(item.get("description")),
            });
        }
    }
    Ok(env)
}

fn parse_postman_collection_value(root: &Value) -> Result<ImportedProjectBundle, CoreError> {
    if !is_postman_collection(root) {
        return Err(CoreError::Validation {
            reason: "JSON is not a valid Postman v2.0/v2.1 collection.".to_string(),
        });
    }

    let info = root.get("info");
    let title = info
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported Postman Collection");
    let description = extract_description(info.and_then(|v| v.get("description")));

    let mut project = ProjectDoc::new(generate_id("proj"), title);
    project.description = description;
    project.auth = parse_postman_auth(root.get("auth"), AuthConfig::None);

    let mut environments = Vec::new();
    let collection_vars = parse_variable_list(root.get("variable"));
    if !collection_vars.is_empty() {
        let mut env = EnvironmentDoc::new(generate_id("env"), "Collection Variables");
        env.variables = collection_vars;
        environments.push(env);
    }

    let mut root_requests = Vec::new();
    let mut folders = Vec::new();
    let mut order_counter = 0i32;

    if let Some(items) = root.get("item").and_then(Value::as_array) {
        parse_postman_items(
            items,
            &mut root_requests,
            &mut folders,
            &mut order_counter,
        );
    }

    Ok(ImportedProjectBundle {
        project,
        environments,
        folders,
        root_requests,
    })
}

fn parse_postman_items(
    items: &[Value],
    requests_out: &mut Vec<HttpRequestDoc>,
    folders_out: &mut Vec<ImportedFolder>,
    order_counter: &mut i32,
) {
    for (ix, item) in items.iter().enumerate() {
        if item.get("request").is_some() {
            if let Some(mut req) = parse_postman_request_item(item) {
                req.order = *order_counter;
                *order_counter += 1;
                requests_out.push(req);
            }
        } else if let Some(child_items) = item.get("item").and_then(Value::as_array) {
            let folder_name = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Folder");
            let mut folder_doc = FolderDoc::new(generate_id("fld"), folder_name);
            folder_doc.description = extract_description(item.get("description"));
            folder_doc.auth = parse_postman_auth(item.get("auth"), AuthConfig::Inherit);
            folder_doc.order = ix as i32;

            let mut child_requests = Vec::new();
            let mut child_folders = Vec::new();
            parse_postman_items(
                child_items,
                &mut child_requests,
                &mut child_folders,
                order_counter,
            );
            folders_out.push(ImportedFolder {
                doc: folder_doc,
                requests: child_requests,
                folders: child_folders,
            });
        }
    }
}

fn parse_postman_request_item(item: &Value) -> Option<HttpRequestDoc> {
    let req_val = item.get("request")?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Untitled Request");

    let (method, url_val, headers_val, body_val, auth_val, req_desc) =
        if let Some(url_str) = req_val.as_str() {
            (
                HttpMethod::Get,
                Value::String(url_str.to_string()),
                None,
                None,
                None,
                String::new(),
            )
        } else {
            let method = req_val
                .get("method")
                .and_then(Value::as_str)
                .map(parse_http_method)
                .unwrap_or(HttpMethod::Get);
            (
                method,
                req_val.get("url").cloned().unwrap_or(Value::Null),
                req_val.get("header"),
                req_val.get("body"),
                req_val.get("auth"),
                extract_description(req_val.get("description")),
            )
        };

    let (url, query_params) = parse_postman_url(&url_val);
    let mut req = HttpRequestDoc::new(generate_id("req"), name, method, url);
    req.description = if req_desc.is_empty() {
        extract_description(item.get("description"))
    } else {
        req_desc
    };
    req.request.params = query_params;
    req.request.headers = parse_postman_kv_rows(headers_val);
    req.request.body = parse_postman_body(body_val);
    req.request.auth = parse_postman_auth(auth_val, AuthConfig::Inherit);
    req.request.post_script = extract_postman_test_script(item.get("event"));
    Some(req)
}

fn parse_http_method(raw: &str) -> HttpMethod {
    match raw.trim().to_ascii_uppercase().as_str() {
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        _ => HttpMethod::Get,
    }
}

fn parse_postman_url(url_val: &Value) -> (String, Vec<KeyValueRow>) {
    if let Some(s) = url_val.as_str() {
        return split_raw_url_query(s);
    }
    let Some(obj) = url_val.as_object() else {
        return (String::new(), Vec::new());
    };

    let query_rows = parse_postman_kv_rows(obj.get("query"));
    if let Some(raw) = obj.get("raw").and_then(Value::as_str) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            if query_rows.is_empty() {
                return split_raw_url_query(trimmed);
            }
            let base = trimmed
                .split_once('?')
                .map(|(b, _)| b)
                .unwrap_or(trimmed)
                .to_string();
            return (base, query_rows);
        }
    }

    let protocol = obj
        .get("protocol")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let host = join_string_array(obj.get("host"), ".");
    let port = obj
        .get("port")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let path = join_string_array(obj.get("path"), "/");

    let mut built = String::new();
    if let Some(proto) = protocol {
        built.push_str(proto.trim());
        built.push_str("://");
    }
    built.push_str(&host);
    if let Some(p) = port {
        built.push(':');
        built.push_str(p.trim());
    }
    if !path.is_empty() {
        if !built.is_empty() && !built.ends_with('/') && !path.starts_with('/') {
            built.push('/');
        }
        built.push_str(&path);
    }

    (built, query_rows)
}

fn split_raw_url_query(raw: &str) -> (String, Vec<KeyValueRow>) {
    let trimmed = raw.trim();
    let Some((base, query_str)) = trimmed.split_once('?') else {
        return (trimmed.to_string(), Vec::new());
    };
    let mut rows = Vec::new();
    for pair in query_str.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if !k.is_empty() {
            rows.push(KeyValueRow::new(k, v));
        }
    }
    (base.to_string(), rows)
}

fn join_string_array(val: Option<&Value>, sep: &str) -> String {
    val.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(sep)
        })
        .unwrap_or_default()
}

fn parse_postman_kv_rows(val: Option<&Value>) -> Vec<KeyValueRow> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let key = item.get("key").and_then(Value::as_str)?.trim();
            if key.is_empty() {
                return None;
            }
            let value = value_to_string(item.get("value"));
            let disabled = item
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let description = extract_description(item.get("description"));
            Some(KeyValueRow {
                name: key.to_string(),
                value,
                enabled: !disabled,
                description,
            })
        })
        .collect()
}

fn parse_postman_body(body_val: Option<&Value>) -> RequestBody {
    let Some(obj) = body_val.and_then(Value::as_object) else {
        return RequestBody::None;
    };
    let mode = obj
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    match mode {
        "raw" => {
            let raw = obj
                .get("raw")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let lang = obj
                .get("options")
                .and_then(|o| o.get("raw"))
                .and_then(|r| r.get("language"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let trimmed = raw.trim_start();
            if lang == "json"
                || (lang.is_empty() && (trimmed.starts_with('{') || trimmed.starts_with('[')))
            {
                RequestBody::Json { content: raw }
            } else {
                let raw_format = match lang.as_str() {
                    "xml" => RawBodyFormat::Xml,
                    "html" => RawBodyFormat::Html,
                    "javascript" => RawBodyFormat::Javascript,
                    _ => RawBodyFormat::Text,
                };
                RequestBody::Raw {
                    raw_format,
                    content: raw,
                }
            }
        }
        "urlencoded" => {
            let fields = parse_postman_kv_rows(obj.get("urlencoded"));
            RequestBody::FormUrlEncoded { fields }
        }
        "formdata" => {
            let mut fields = Vec::new();
            if let Some(arr) = obj.get("formdata").and_then(Value::as_array) {
                for item in arr {
                    let Some(key) = item.get("key").and_then(Value::as_str).map(str::trim) else {
                        continue;
                    };
                    if key.is_empty() {
                        continue;
                    }
                    let disabled = item
                        .get("disabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let field_type = item
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("text");
                    let description = extract_description(item.get("description"));
                    let mut row = if field_type.eq_ignore_ascii_case("file") {
                        let file_path = item
                            .get("src")
                            .and_then(|s| {
                                s.as_str().map(str::to_string).or_else(|| {
                                    s.as_array()
                                        .and_then(|a| a.first())
                                        .and_then(Value::as_str)
                                        .map(str::to_string)
                                })
                            })
                            .unwrap_or_default();
                        MultipartField::file(key, file_path)
                    } else {
                        MultipartField::text(key, value_to_string(item.get("value")))
                    };
                    row.enabled = !disabled;
                    row.description = description;
                    fields.push(row);
                }
            }
            RequestBody::Multipart { fields }
        }
        "graphql" => {
            let gql = obj.get("graphql").cloned().unwrap_or(Value::Null);
            let content = serde_json::to_string_pretty(&gql).unwrap_or_default();
            RequestBody::Json { content }
        }
        _ => RequestBody::None,
    }
}

fn parse_postman_auth(auth_val: Option<&Value>, default_auth: AuthConfig) -> AuthConfig {
    let Some(obj) = auth_val.and_then(Value::as_object) else {
        return default_auth;
    };
    if obj.is_empty() {
        return default_auth;
    }
    let Some(auth_type) = obj.get("type").and_then(Value::as_str) else {
        return default_auth;
    };
    match auth_type {
        "noauth" => AuthConfig::None,
        "bearer" => {
            let token = find_postman_auth_attr(obj.get("bearer"), "token");
            AuthConfig::Bearer {
                token,
                prefix: "Bearer".to_string(),
            }
        }
        "basic" => {
            let username = find_postman_auth_attr(obj.get("basic"), "username");
            let password = find_postman_auth_attr(obj.get("basic"), "password");
            AuthConfig::Basic { username, password }
        }
        "apikey" => {
            let name = find_postman_auth_attr(obj.get("apikey"), "key");
            let value = find_postman_auth_attr(obj.get("apikey"), "value");
            let in_loc = find_postman_auth_attr(obj.get("apikey"), "in");
            let location = if in_loc.eq_ignore_ascii_case("query") {
                ApiKeyLocation::Query
            } else {
                ApiKeyLocation::Header
            };
            AuthConfig::ApiKey {
                key: name,
                value,
                location,
            }
        }
        _ => default_auth,
    }
}

fn find_postman_auth_attr(attr_val: Option<&Value>, target_key: &str) -> String {
    if let Some(arr) = attr_val.and_then(Value::as_array) {
        for item in arr {
            if item
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|k| k == target_key)
            {
                return value_to_string(item.get("value"));
            }
        }
    } else if let Some(obj) = attr_val.and_then(Value::as_object) {
        return value_to_string(obj.get(target_key));
    }
    String::new()
}

fn parse_variable_list(val: Option<&Value>) -> Vec<EnvVariable> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let key = item.get("key").and_then(Value::as_str)?.trim();
            if key.is_empty() {
                return None;
            }
            let disabled = item
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let secret = item
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.eq_ignore_ascii_case("secret"));
            Some(EnvVariable {
                key: key.to_string(),
                value: value_to_string(item.get("value")),
                enabled: !disabled,
                secret,
                description: extract_description(item.get("description")),
            })
        })
        .collect()
}

fn extract_postman_test_script(events_val: Option<&Value>) -> String {
    let Some(arr) = events_val.and_then(Value::as_array) else {
        return String::new();
    };
    let mut lines = Vec::new();
    for ev in arr {
        if ev.get("listen").and_then(Value::as_str) == Some("test")
            && let Some(exec) = ev
                .get("script")
                .and_then(|s| s.get("exec"))
                .and_then(Value::as_array)
        {
            for line in exec {
                if let Some(s) = line.as_str() {
                    lines.push(s);
                }
            }
        }
    }
    lines.join("\n").trim().to_string()
}

fn extract_description(val: Option<&Value>) -> String {
    match val {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Object(obj)) => obj
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

fn value_to_string(val: Option<&Value>) -> String {
    match val {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    #[test]
    fn parses_postman_zip_with_collection_and_environment() {
        let collection_json = r#"{
          "info": {
            "name": "Demo Postman API",
            "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
          },
          "item": [
            {
              "name": "Users",
              "item": [
                {
                  "name": "Nested",
                  "item": [
                    {
                      "name": "Create User",
                      "request": {
                        "method": "POST",
                        "header": [{"key": "X-App", "value": "testrix"}],
                        "body": {
                          "mode": "raw",
                          "raw": "{\"name\":\"Ada\"}",
                          "options": {"raw": {"language": "json"}}
                        },
                        "url": {
                          "raw": "{{baseUrl}}/users?role=admin",
                          "query": [{"key": "role", "value": "admin"}]
                        }
                      }
                    }
                  ]
                }
              ]
            }
          ]
        }"#;

        let env_json = r#"{
          "name": "Local 231",
          "_postman_variable_scope": "environment",
          "values": [
            {"key": "baseUrl", "value": "http://localhost:8080", "enabled": true}
          ]
        }"#;

        let mut buf = Vec::new();
        {
            let mut writer = ZipArchiveWriter::new(Cursor::new(&mut buf));
            writer
                .start_file("demo.postman_collection.json", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(collection_json.as_bytes()).unwrap();
            writer
                .start_file("local.postman_environment.json", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(env_json.as_bytes()).unwrap();
            writer.finish().unwrap();
        }

        let bundles = parse_postman_zip(&buf).expect("parse postman zip");
        assert_eq!(bundles.len(), 1);
        let bundle = &bundles[0];
        assert_eq!(bundle.project.name, "Demo Postman API");
        assert_eq!(bundle.environments.len(), 1);
        assert_eq!(bundle.environments[0].name, "Local 231");
        assert_eq!(bundle.environments[0].variables[0].key, "baseUrl");
        assert_eq!(bundle.folders.len(), 1);
        assert_eq!(bundle.folders[0].doc.name, "Users");
        assert_eq!(bundle.folders[0].folders.len(), 1);
        assert_eq!(bundle.folders[0].folders[0].doc.name, "Nested");
        let req = &bundle.folders[0].folders[0].requests[0];
        assert_eq!(req.name, "Create User");
        assert_eq!(req.request.method, HttpMethod::Post);
        assert_eq!(req.request.url, "{{baseUrl}}/users");
        assert_eq!(req.request.params.len(), 1);
        assert_eq!(req.request.headers.len(), 1);
        assert!(matches!(req.request.body, RequestBody::Json { .. }));
    }

    use zip::ZipWriter as ZipArchiveWriter;
}
