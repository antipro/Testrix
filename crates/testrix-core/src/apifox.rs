use serde_json::{Map, Value};

use crate::{
    error::CoreError,
    model::{
        ApiKeyLocation, AuthConfig, EnvVariable, EnvironmentDoc, FolderDoc, HttpMethod,
        HttpRequestDoc, KeyValueRow, MultipartField, ProjectDoc, RawBodyFormat, RequestBody,
        generate_id,
    },
    openapi::{ImportedFolder, ImportedProjectBundle, parse_openapi_3_0},
    response::ResponseHeader,
    response_meta::{
        ResponseExample, ResponseMetaField, extract_and_merge_response_meta,
        json_schema_to_meta_fields, merge_response_meta,
    },
};

/// Parses an Apifox native JSON export (`apifoxProject` / `apiCollection`) or an Apifox OpenAPI 3.0
/// JSON export into an [`ImportedProjectBundle`].
pub fn parse_apifox(json_str: &str) -> Result<ImportedProjectBundle, CoreError> {
    let root: Value = serde_json::from_str(json_str).map_err(|err| CoreError::Validation {
        reason: format!("Invalid Apifox JSON: {err}"),
    })?;

    if root.get("openapi").is_some() {
        return parse_openapi_3_0(json_str);
    }

    if !is_apifox_native(&root) {
        return Err(CoreError::Validation {
            reason: "JSON is neither a native Apifox export (apifoxProject/apiCollection) nor an OpenAPI 3.0 document.".to_string(),
        });
    }

    parse_apifox_native_value(&root)
}

fn is_apifox_native(root: &Value) -> bool {
    root.get("apifoxProject").is_some()
        || root
            .get("apiCollection")
            .and_then(Value::as_array)
            .is_some()
}

fn parse_apifox_native_value(root: &Value) -> Result<ImportedProjectBundle, CoreError> {
    let info = root.get("info");
    let title = info
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported Apifox Project");
    let description = info
        .and_then(|v| v.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    let mut project = ProjectDoc::new(generate_id("proj"), title);
    project.description = description;

    let environments = parse_apifox_environments(root.get("environments"));
    let has_base_url = environments
        .iter()
        .any(|env| env.variables.iter().any(|v| v.key == "base_url" && !v.value.is_empty()));
    if has_base_url {
        project.base_url = "{{base_url}}".to_string();
    }

    let mut root_requests = Vec::new();
    let mut folders = Vec::new();
    let mut order_counter = 0i32;

    if let Some(collection) = root.get("apiCollection").and_then(Value::as_array) {
        parse_apifox_collection_nodes(
            collection,
            has_base_url,
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

fn parse_apifox_environments(val: Option<&Value>) -> Vec<EnvironmentDoc> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut envs = Vec::new();
    for item in arr {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Default");
        let mut env = EnvironmentDoc::new(generate_id("env"), name);

        let base_url = item
            .get("baseUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                item.get("baseUrls")
                    .and_then(Value::as_array)
                    .and_then(|urls| urls.first())
                    .and_then(|u| u.get("url").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            });
        if let Some(url) = base_url {
            env.variables.push(EnvVariable {
                key: "base_url".to_string(),
                value: url.to_string(),
                enabled: true,
                secret: false,
                description: "Apifox environment base URL".to_string(),
            });
        }

        for list_key in ["parameters", "variables"] {
            if let Some(vars) = item.get(list_key).and_then(Value::as_array) {
                for v in vars {
                    let key = v
                        .get("name")
                        .or_else(|| v.get("key"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default();
                    if key.is_empty()
                        || env.variables.iter().any(|existing| existing.key == key)
                    {
                        continue;
                    }
                    let value = value_to_string(v.get("value").or_else(|| v.get("currentValue")));
                    let enabled = v
                        .get("enable")
                        .or_else(|| v.get("enabled"))
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let secret = v
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.eq_ignore_ascii_case("secret"));
                    let description = v
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    env.variables.push(EnvVariable {
                        key: key.to_string(),
                        value,
                        enabled,
                        secret,
                        description,
                    });
                }
            }
        }

        envs.push(env);
    }
    envs
}

fn parse_apifox_collection_nodes(
    nodes: &[Value],
    has_base_url: bool,
    requests_out: &mut Vec<HttpRequestDoc>,
    folders_out: &mut Vec<ImportedFolder>,
    order_counter: &mut i32,
) {
    for (ix, node) in nodes.iter().enumerate() {
        if let Some(api_val) = node.get("api") {
            if let Some(mut req) = parse_apifox_api(node, api_val, has_base_url) {
                req.order = *order_counter;
                *order_counter += 1;
                requests_out.push(req);
            }
        } else if let Some(items) = node.get("items").and_then(Value::as_array) {
            let folder_name = node
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("Folder");
            let mut folder_doc = FolderDoc::new(generate_id("fld"), folder_name);
            folder_doc.description = node
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            folder_doc.auth = parse_apifox_auth(node.get("auth"), AuthConfig::Inherit);
            folder_doc.order = ix as i32;

            let mut child_requests = Vec::new();
            let mut child_folders = Vec::new();
            parse_apifox_collection_nodes(
                items,
                has_base_url,
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

fn parse_apifox_api(node: &Value, api: &Value, has_base_url: bool) -> Option<HttpRequestDoc> {
    let name = node
        .get("name")
        .or_else(|| api.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Untitled Request");

    let method = api
        .get("method")
        .and_then(Value::as_str)
        .map(parse_http_method)
        .unwrap_or(HttpMethod::Get);

    let raw_path = api
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("/")
        .trim();
    let url = if raw_path.starts_with("http://")
        || raw_path.starts_with("https://")
        || raw_path.starts_with("{{")
    {
        raw_path.to_string()
    } else if has_base_url {
        let normalized = if raw_path.starts_with('/') {
            raw_path.to_string()
        } else {
            format!("/{raw_path}")
        };
        format!("{{{{base_url}}}}{normalized}")
    } else {
        raw_path.to_string()
    };

    let mut req = HttpRequestDoc::new(generate_id("req"), name, method, url);
    req.description = api
        .get("description")
        .or_else(|| node.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    if let Some(params_obj) = api.get("parameters").and_then(Value::as_object) {
        req.request.params = parse_apifox_kv_list(params_obj.get("query"));
        req.request.headers = parse_apifox_kv_list(params_obj.get("header"));
    }

    req.request.body = parse_apifox_body(api.get("requestBody"));
    req.request.auth = parse_apifox_auth(api.get("auth"), AuthConfig::Inherit);
    req.request.post_script = extract_apifox_post_script(api.get("postProcessors"));
    let (examples, meta) =
        parse_apifox_responses(api.get("responses"), api.get("responseExamples"));
    req.request.response_examples = examples;
    req.request.response_meta = meta;

    Some(req)
}

fn parse_apifox_responses(
    responses_val: Option<&Value>,
    examples_val: Option<&Value>,
) -> (Vec<ResponseExample>, Vec<ResponseMetaField>) {
    let mut examples = Vec::new();
    let mut meta = Vec::new();

    struct RespDef {
        id: Option<String>,
        name: String,
        code: u16,
        content_type: Option<String>,
        headers: Vec<ResponseHeader>,
        body_fallback: Option<String>,
    }

    let mut defs = Vec::new();
    if let Some(arr) = responses_val.and_then(Value::as_array) {
        for (ix, resp) in arr.iter().enumerate() {
            let id = resp
                .get("id")
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                });
            let name = resp
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Response {}", ix + 1));
            let code = resp
                .get("code")
                .and_then(Value::as_u64)
                .and_then(|c| u16::try_from(c).ok())
                .unwrap_or(200);
            let content_type = resp
                .get("contentType")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|ct| match ct.to_ascii_lowercase().as_str() {
                    "json" => "application/json".to_string(),
                    "xml" => "application/xml".to_string(),
                    "html" => "text/html".to_string(),
                    _ => ct.to_string(),
                });
            let headers = resp
                .get("headers")
                .and_then(Value::as_array)
                .map(|hdrs| {
                    hdrs.iter()
                        .filter_map(|h| {
                            let h_name = h.get("name").and_then(Value::as_str)?.trim();
                            if h_name.is_empty() {
                                return None;
                            }
                            let val = value_to_string(
                                h.get("example")
                                    .or_else(|| h.get("value"))
                                    .or_else(|| h.get("defaultValue")),
                            );
                            Some(ResponseHeader {
                                name: h_name.to_string(),
                                value: val,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            if let Some(schema) = resp.get("jsonSchema") {
                let schema_meta = json_schema_to_meta_fields(schema, None, 0);
                if !schema_meta.is_empty() {
                    merge_response_meta(&mut meta, schema_meta);
                }
            }

            let body_fallback = resp
                .get("example")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    resp.get("jsonSchema")
                        .map(synthesize_json_example)
                        .and_then(|v| serde_json::to_string_pretty(&v).ok())
                });

            defs.push(RespDef {
                id,
                name,
                code,
                content_type,
                headers,
                body_fallback,
            });
        }
    }

    if let Some(ex_arr) = examples_val.and_then(Value::as_array) {
        for (ix, ex) in ex_arr.iter().enumerate() {
            let resp_id = ex.get("responseId").and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
            });
            let matched_def = resp_id
                .as_deref()
                .and_then(|id| defs.iter().find(|d| d.id.as_deref() == Some(id)));
            let name = ex
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| matched_def.map(|d| d.name.clone()))
                .unwrap_or_else(|| format!("Example {}", ix + 1));
            let code = ex
                .get("code")
                .and_then(Value::as_u64)
                .and_then(|c| u16::try_from(c).ok())
                .or_else(|| matched_def.map(|d| d.code))
                .unwrap_or(200);
            let content_type = matched_def
                .and_then(|d| d.content_type.clone())
                .or_else(|| Some("application/json".to_string()));
            let headers = matched_def.map(|d| d.headers.clone()).unwrap_or_default();
            let body = match ex.get("data") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => {
                    serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string())
                }
                None => String::new(),
            };
            extract_and_merge_response_meta(&mut meta, &body, content_type.as_deref());
            examples.push(ResponseExample::new(
                generate_id("ex"),
                name,
                code,
                String::new(),
                content_type,
                headers,
                body,
            ));
        }
    }

    if examples.is_empty() {
        for def in defs {
            if let Some(body) = def.body_fallback {
                extract_and_merge_response_meta(&mut meta, &body, def.content_type.as_deref());
                examples.push(ResponseExample::new(
                    generate_id("ex"),
                    def.name,
                    def.code,
                    String::new(),
                    def.content_type,
                    def.headers,
                    body,
                ));
            }
        }
    }

    (examples, meta)
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

fn parse_apifox_kv_list(val: Option<&Value>) -> Vec<KeyValueRow> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(Value::as_str)?.trim();
            if name.is_empty() {
                return None;
            }
            let value = value_to_string(
                item.get("example")
                    .or_else(|| item.get("value"))
                    .or_else(|| item.get("defaultValue")),
            );
            let enabled = item
                .get("enable")
                .or_else(|| item.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let description = item
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            Some(KeyValueRow {
                name: name.to_string(),
                value,
                enabled,
                description,
            })
        })
        .collect()
}

fn parse_apifox_body(val: Option<&Value>) -> RequestBody {
    let Some(obj) = val.and_then(Value::as_object) else {
        return RequestBody::None;
    };
    let body_type = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .trim()
        .to_ascii_lowercase();

    match body_type.as_str() {
        "" | "none" => RequestBody::None,
        "application/json" | "json" => {
            let example = obj
                .get("example")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    obj.get("jsonSchema")
                        .map(synthesize_json_example)
                        .and_then(|v| serde_json::to_string_pretty(&v).ok())
                })
                .unwrap_or_else(|| "{}".to_string());
            RequestBody::Json { content: example }
        }
        "application/x-www-form-urlencoded" => RequestBody::FormUrlEncoded {
            fields: parse_apifox_kv_list(obj.get("parameters")),
        },
        "multipart/form-data" => {
            let mut fields = Vec::new();
            if let Some(arr) = obj.get("parameters").and_then(Value::as_array) {
                for item in arr {
                    let Some(name) = item.get("name").and_then(Value::as_str).map(str::trim) else {
                        continue;
                    };
                    if name.is_empty() {
                        continue;
                    }
                    let is_file = item
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.eq_ignore_ascii_case("file"));
                    let enabled = item
                        .get("enable")
                        .or_else(|| item.get("enabled"))
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let value = value_to_string(item.get("example").or_else(|| item.get("value")));
                    let description = item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    let mut field = if is_file {
                        MultipartField::file(name, value)
                    } else {
                        MultipartField::text(name, value)
                    };
                    field.enabled = enabled;
                    field.description = description;
                    fields.push(field);
                }
            }
            RequestBody::Multipart { fields }
        }
        "application/xml" | "text/xml" | "xml" => RequestBody::Raw {
            raw_format: RawBodyFormat::Xml,
            content: obj
                .get("example")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "text/html" | "html" => RequestBody::Raw {
            raw_format: RawBodyFormat::Html,
            content: obj
                .get("example")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        _ => RequestBody::Raw {
            raw_format: RawBodyFormat::Text,
            content: obj
                .get("example")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
    }
}

fn synthesize_json_example(schema: &Value) -> Value {
    if let Some(example) = schema.get("example") {
        return example.clone();
    }
    let schema_type = schema.get("type").and_then(Value::as_str).unwrap_or("");
    match schema_type {
        "object" => {
            let mut map = Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (k, v) in props {
                    map.insert(k.clone(), synthesize_json_example(v));
                }
            }
            Value::Object(map)
        }
        "array" => {
            let item = schema
                .get("items")
                .map(synthesize_json_example)
                .unwrap_or(Value::Null);
            Value::Array(vec![item])
        }
        "integer" | "number" => Value::Number(0.into()),
        "boolean" => Value::Bool(false),
        _ => Value::String(String::new()),
    }
}

fn parse_apifox_auth(val: Option<&Value>, default_auth: AuthConfig) -> AuthConfig {
    let Some(obj) = val.and_then(Value::as_object) else {
        return default_auth;
    };
    let auth_type = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("inherit")
        .trim()
        .to_ascii_lowercase();
    match auth_type.as_str() {
        "noauth" | "none" => AuthConfig::None,
        "bearer" => {
            let token = obj
                .get("bearer")
                .and_then(|b| b.get("token"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            AuthConfig::Bearer {
                token,
                prefix: "Bearer".to_string(),
            }
        }
        "basic" => {
            let basic = obj.get("basic");
            let username = basic
                .and_then(|b| b.get("username"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let password = basic
                .and_then(|b| b.get("password"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            AuthConfig::Basic { username, password }
        }
        "apikey" => {
            let apikey = obj.get("apikey");
            let key = apikey
                .and_then(|a| a.get("key"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let value = apikey
                .and_then(|a| a.get("value"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let location = if apikey
                .and_then(|a| a.get("in"))
                .and_then(Value::as_str)
                .is_some_and(|loc| loc.eq_ignore_ascii_case("query"))
            {
                ApiKeyLocation::Query
            } else {
                ApiKeyLocation::Header
            };
            AuthConfig::ApiKey {
                key,
                value,
                location,
            }
        }
        _ => default_auth,
    }
}

fn extract_apifox_post_script(val: Option<&Value>) -> String {
    let Some(arr) = val.and_then(Value::as_array) else {
        return String::new();
    };
    let mut scripts = Vec::new();
    for item in arr {
        let is_script = item
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t == "customScript");
        let enabled = item
            .get("enable")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if is_script
            && enabled
            && let Some(data) = item
                .get("data")
                .and_then(|d| d.get("data"))
                .and_then(Value::as_str)
                .or_else(|| item.get("data").and_then(Value::as_str))
        {
            let trimmed = data.trim();
            if !trimmed.is_empty() {
                scripts.push(trimmed);
            }
        }
    }
    scripts.join("\n")
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
    use super::*;

    #[test]
    fn parses_native_apifox_export_into_project_bundle() {
        let json = r#"{
          "apifoxProject": "1.0.0",
          "info": {
            "name": "Apifox Native Demo",
            "description": "Exported from Apifox"
          },
          "environments": [
            {
              "name": "Dev",
              "baseUrl": "https://api.example.com",
              "parameters": [
                {"name": "token", "value": "abc", "enable": true}
              ]
            }
          ],
          "apiCollection": [
            {
              "name": "Orders",
              "items": [
                {
                  "name": "Create Order",
                  "api": {
                    "method": "post",
                    "path": "/orders",
                    "parameters": {
                      "query": [{"name": "dryRun", "example": "true", "enable": true}],
                      "header": [{"name": "X-Trace", "example": "123", "enable": true}]
                    },
                    "requestBody": {
                      "type": "application/json",
                      "jsonSchema": {
                        "type": "object",
                        "properties": {
                          "amount": {"type": "number"}
                        }
                      }
                    },
                    "auth": {
                      "type": "bearer",
                      "bearer": {"token": "{{token}}"}
                    },
                    "responses": [
                      {
                        "id": "resp-200",
                        "name": "Success",
                        "code": 200,
                        "contentType": "json",
                        "jsonSchema": {
                          "type": "object",
                          "properties": {
                            "orderId": {"type": "string", "description": "Created order ID"}
                          }
                        }
                      }
                    ],
                    "responseExamples": [
                      {
                        "name": "OK Example",
                        "responseId": "resp-200",
                        "data": "{\"orderId\":\"ord_123\"}"
                      }
                    ]
                  }
                }
              ]
            }
          ]
        }"#;

        let bundle = parse_apifox(json).expect("parse apifox");
        assert_eq!(bundle.project.name, "Apifox Native Demo");
        assert_eq!(bundle.project.base_url, "{{base_url}}");
        assert_eq!(bundle.environments.len(), 1);
        assert_eq!(bundle.environments[0].name, "Dev");
        assert_eq!(bundle.environments[0].variables.len(), 2);
        assert_eq!(bundle.folders.len(), 1);
        assert_eq!(bundle.folders[0].doc.name, "Orders");
        let req = &bundle.folders[0].requests[0];
        assert_eq!(req.name, "Create Order");
        assert_eq!(req.request.method, HttpMethod::Post);
        assert_eq!(req.request.url, "{{base_url}}/orders");
        assert_eq!(req.request.params.len(), 1);
        assert_eq!(req.request.headers.len(), 1);
        assert!(matches!(req.request.body, RequestBody::Json { .. }));
        assert!(matches!(req.request.auth, AuthConfig::Bearer { .. }));
        assert_eq!(req.request.response_examples.len(), 1);
        assert_eq!(req.request.response_examples[0].name, "OK Example");
        assert_eq!(req.request.response_examples[0].status_code, 200);
        assert_eq!(req.request.response_meta.len(), 1);
        assert_eq!(req.request.response_meta[0].name, "orderId");
        assert_eq!(req.request.response_meta[0].description, "Created order ID");
    }
}
