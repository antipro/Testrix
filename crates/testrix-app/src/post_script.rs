use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Mutex, OnceLock},
};

use anyhow::{Context as _, Result};
use gpui_kit::{
    App, Task, Window,
    component::input::{CompletionProvider, RopeExt as _},
};
use gpui_shell::{HostModule, HostValue, ShellRuntime};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Documentation, Range, TextEdit,
};
use ropey::Rope;
use serde::Serialize;
use testrix_core::{EnvVariable, EnvironmentDoc, HttpResponseSnapshot};

#[derive(Default)]
struct BridgeState {
    context_json: String,
    committed_vars_json: Option<String>,
}

static RUN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static BRIDGE_STATE: OnceLock<Mutex<BridgeState>> = OnceLock::new();

fn register_post_script_module_on_current_thread() -> Result<()> {
    gpui_shell::export_module(
        HostModule::new("testrix-post-script")
            .function("__get_context", |_| {
                let state = BRIDGE_STATE
                    .get_or_init(|| Mutex::new(BridgeState::default()))
                    .lock()
                    .map_err(|_| gpui_shell::HostError::new("post-script state poisoned"))?;
                Ok(HostValue::from(state.context_json.clone()))
            })
            .function("__commit_env", |args| {
                let json = args.string(0)?.to_owned();
                let mut state = BRIDGE_STATE
                    .get_or_init(|| Mutex::new(BridgeState::default()))
                    .lock()
                    .map_err(|_| gpui_shell::HostError::new("post-script state poisoned"))?;
                state.committed_vars_json = Some(json);
                Ok(HostValue::Null)
            }),
    )
    .map_err(|err| anyhow::anyhow!("failed to register testrix-post-script module: {err}"))
}

#[derive(Serialize)]
struct ScriptEnvironment<'a> {
    id: &'a str,
    name: &'a str,
    var: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct ScriptRequest<'a> {
    id: &'a str,
    name: &'a str,
    method: &'a str,
    url: &'a str,
    headers: BTreeMap<String, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScriptResponse<'a> {
    status: u16,
    reason: &'a str,
    headers: BTreeMap<String, String>,
    body: &'a str,
    duration_ms: u64,
    size_bytes: usize,
}

#[derive(Serialize)]
struct ScriptContext<'a> {
    environment: ScriptEnvironment<'a>,
    request: ScriptRequest<'a>,
    response: ScriptResponse<'a>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PostScriptResult {
    pub changed_keys: Vec<String>,
    pub next_vars: BTreeMap<String, String>,
}

/// Executes a request's `postScript` using `gpui-shell` (QuickJS), exposing
/// `environment`, `request`, and `response` globals.
pub(crate) fn run_post_script(
    script: &str,
    request_id: &str,
    request_name: &str,
    active_env: Option<&EnvironmentDoc>,
    snapshot: &HttpResponseSnapshot,
) -> Result<PostScriptResult> {
    if script.trim().is_empty() {
        return Ok(PostScriptResult::default());
    }

    let _run_guard = RUN_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("post-script runner lock poisoned"))?;

    let initial_vars = testrix_core::env_variable_map(active_env);
    let mut req_headers = BTreeMap::new();
    for h in &snapshot.sent_request.headers {
        req_headers.insert(h.name.clone(), h.value.clone());
    }
    let mut res_headers = BTreeMap::new();
    for h in &snapshot.headers {
        res_headers.insert(h.name.clone(), h.value.clone());
    }

    let ctx_payload = ScriptContext {
        environment: ScriptEnvironment {
            id: active_env.map_or("", |e| e.id.as_str()),
            name: active_env.map_or("", |e| e.name.as_str()),
            var: initial_vars.clone(),
        },
        request: ScriptRequest {
            id: request_id,
            name: request_name,
            method: snapshot.sent_request.method.as_str(),
            url: &snapshot.sent_request.url,
            headers: req_headers,
        },
        response: ScriptResponse {
            status: snapshot.status_code,
            reason: &snapshot.reason,
            headers: res_headers,
            body: &snapshot.body_text,
            duration_ms: snapshot.duration_ms,
            size_bytes: snapshot.size_bytes,
        },
    };
    let context_json = serde_json::to_string(&ctx_payload)?;

    {
        let mut state = BRIDGE_STATE
            .get_or_init(|| Mutex::new(BridgeState::default()))
            .lock()
            .map_err(|_| anyhow::anyhow!("post-script state poisoned"))?;
        state.context_json = context_json;
        state.committed_vars_json = None;
    }

    let temp_dir = tempfile::tempdir().context("create temp dir for post-script")?;
    let wrapped_js = format!(
        r#"import {{ View }} from "gpui-kit";
import {{ __get_context, __commit_env }} from "testrix-post-script";

const __ctx = JSON.parse(__get_context());
globalThis.environment = __ctx.environment;
globalThis.request = __ctx.request;
globalThis.response = __ctx.response;
globalThis.response.json = () => JSON.parse(globalThis.response.body);

try {{
{script}
}} finally {{
  const __vars = (globalThis.environment && globalThis.environment.var) ? globalThis.environment.var : {{}};
  __commit_env(JSON.stringify(__vars));
}}

export default class PostScriptRunner extends View {{
  render() {{ return null; }}
}}
"#
    );
    std::fs::write(temp_dir.path().join("main.js"), wrapped_js)
        .context("write main.js for post-script")?;

    let temp_dir_path = temp_dir.path().to_path_buf();
    let handle = std::thread::Builder::new()
        .name("testrix-post-script".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || -> Result<()> {
            register_post_script_module_on_current_thread()?;
            let runtime = ShellRuntime::new_isolated().context("create gpui-shell runtime")?;
            runtime
                .load_application(&temp_dir_path, "main.js")
                .context("execute post-script")?;
            Ok(())
        })
        .context("spawn post-script thread")?;

    handle
        .join()
        .map_err(|_| anyhow::anyhow!("post-script thread panicked"))??;

    let committed_json = {
        let mut state = BRIDGE_STATE
            .get_or_init(|| Mutex::new(BridgeState::default()))
            .lock()
            .map_err(|_| anyhow::anyhow!("post-script state poisoned"))?;
        state.committed_vars_json.take()
    };

    let next_vars: BTreeMap<String, String> = match committed_json {
        Some(json) => {
            let raw: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&json).context("parse committed environment.var")?;
            raw.into_iter()
                .filter_map(|(k, v)| {
                    let s = match v {
                        serde_json::Value::String(s) => s,
                        serde_json::Value::Null => return None,
                        other => other.to_string(),
                    };
                    Some((k, s))
                })
                .collect()
        }
        None => initial_vars.clone(),
    };

    let changed_keys = next_vars
        .iter()
        .filter(|(k, v)| initial_vars.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect();

    Ok(PostScriptResult {
        changed_keys,
        next_vars,
    })
}

/// Applies `next_vars` from a post-script run into `env_doc`, returning keys that were updated or added.
pub(crate) fn apply_vars_to_environment(
    env_doc: &mut EnvironmentDoc,
    next_vars: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed = Vec::new();
    for (key, value) in next_vars {
        if let Some(existing) = env_doc
            .variables
            .iter_mut()
            .find(|v| v.key.trim() == key.as_str())
        {
            if existing.value != *value || !existing.enabled {
                existing.value = value.clone();
                existing.enabled = true;
                changed.push(key.clone());
            }
        } else {
            env_doc
                .variables
                .push(EnvVariable::new(key.clone(), value.clone()));
            changed.push(key.clone());
        }
    }
    changed
}

struct CompletionEntry {
    label: &'static str,
    insert_text: &'static str,
    kind: CompletionItemKind,
    detail: &'static str,
    doc: &'static str,
}

const TOP_LEVEL_COMPLETIONS: &[CompletionEntry] = &[
    CompletionEntry {
        label: "environment",
        insert_text: "environment",
        kind: CompletionItemKind::VARIABLE,
        detail: "ScriptEnvironment",
        doc: "Active environment profile (`id`, `name`, `var`). Mutations to `environment.var` are saved back to `*.env.yaml`.",
    },
    CompletionEntry {
        label: "request",
        insert_text: "request",
        kind: CompletionItemKind::VARIABLE,
        detail: "ScriptRequest",
        doc: "Executed HTTP request metadata (`id`, `name`, `method`, `url`, `headers`).",
    },
    CompletionEntry {
        label: "response",
        insert_text: "response",
        kind: CompletionItemKind::VARIABLE,
        detail: "ScriptResponse",
        doc: "HTTP response snapshot (`status`, `reason`, `headers`, `body`, `durationMs`, `sizeBytes`, `json()`).",
    },
    CompletionEntry {
        label: "JSON",
        insert_text: "JSON",
        kind: CompletionItemKind::CLASS,
        detail: "JSON",
        doc: "Standard JavaScript JSON parser and serializer (`parse`, `stringify`).",
    },
    CompletionEntry {
        label: "console",
        insert_text: "console",
        kind: CompletionItemKind::MODULE,
        detail: "Console",
        doc: "Standard JavaScript console (`log`, `warn`, `error`).",
    },
];

const ENVIRONMENT_COMPLETIONS: &[CompletionEntry] = &[
    CompletionEntry {
        label: "var",
        insert_text: "var",
        kind: CompletionItemKind::PROPERTY,
        detail: "Record<string, string>",
        doc: "Mutable map of environment variables. Changes are persisted to the active `*.env.yaml` profile.",
    },
    CompletionEntry {
        label: "name",
        insert_text: "name",
        kind: CompletionItemKind::PROPERTY,
        detail: "string",
        doc: "Active environment profile display name.",
    },
    CompletionEntry {
        label: "id",
        insert_text: "id",
        kind: CompletionItemKind::PROPERTY,
        detail: "string",
        doc: "Active environment profile ID.",
    },
];

const REQUEST_COMPLETIONS: &[CompletionEntry] = &[
    CompletionEntry {
        label: "id",
        insert_text: "id",
        kind: CompletionItemKind::PROPERTY,
        detail: "string",
        doc: "Request ID.",
    },
    CompletionEntry {
        label: "name",
        insert_text: "name",
        kind: CompletionItemKind::PROPERTY,
        detail: "string",
        doc: "Request display name.",
    },
    CompletionEntry {
        label: "method",
        insert_text: "method",
        kind: CompletionItemKind::PROPERTY,
        detail: "string",
        doc: "HTTP method (e.g. GET, POST).",
    },
    CompletionEntry {
        label: "url",
        insert_text: "url",
        kind: CompletionItemKind::PROPERTY,
        detail: "string",
        doc: "Materialized request URL.",
    },
    CompletionEntry {
        label: "headers",
        insert_text: "headers",
        kind: CompletionItemKind::PROPERTY,
        detail: "Record<string, string>",
        doc: "Sent HTTP request headers.",
    },
];

const RESPONSE_COMPLETIONS: &[CompletionEntry] = &[
    CompletionEntry {
        label: "status",
        insert_text: "status",
        kind: CompletionItemKind::PROPERTY,
        detail: "number",
        doc: "HTTP status code (e.g. 200).",
    },
    CompletionEntry {
        label: "reason",
        insert_text: "reason",
        kind: CompletionItemKind::PROPERTY,
        detail: "string",
        doc: "HTTP status reason phrase (e.g. OK).",
    },
    CompletionEntry {
        label: "headers",
        insert_text: "headers",
        kind: CompletionItemKind::PROPERTY,
        detail: "Record<string, string>",
        doc: "HTTP response headers.",
    },
    CompletionEntry {
        label: "body",
        insert_text: "body",
        kind: CompletionItemKind::PROPERTY,
        detail: "string",
        doc: "Raw HTTP response body text.",
    },
    CompletionEntry {
        label: "durationMs",
        insert_text: "durationMs",
        kind: CompletionItemKind::PROPERTY,
        detail: "number",
        doc: "Request duration in milliseconds.",
    },
    CompletionEntry {
        label: "sizeBytes",
        insert_text: "sizeBytes",
        kind: CompletionItemKind::PROPERTY,
        detail: "number",
        doc: "Response body size in bytes.",
    },
    CompletionEntry {
        label: "json",
        insert_text: "json()",
        kind: CompletionItemKind::METHOD,
        detail: "() => any",
        doc: "Parses `response.body` as JSON (`JSON.parse(response.body)`).",
    },
];

const JSON_COMPLETIONS: &[CompletionEntry] = &[
    CompletionEntry {
        label: "parse",
        insert_text: "parse()",
        kind: CompletionItemKind::METHOD,
        detail: "(text: string) => any",
        doc: "Parses a JSON string into a JavaScript value.",
    },
    CompletionEntry {
        label: "stringify",
        insert_text: "stringify()",
        kind: CompletionItemKind::METHOD,
        detail: "(value: any) => string",
        doc: "Converts a JavaScript value to a JSON string.",
    },
];

const CONSOLE_COMPLETIONS: &[CompletionEntry] = &[
    CompletionEntry {
        label: "log",
        insert_text: "log()",
        kind: CompletionItemKind::METHOD,
        detail: "(...args: any[]) => void",
        doc: "Logs a message.",
    },
    CompletionEntry {
        label: "warn",
        insert_text: "warn()",
        kind: CompletionItemKind::METHOD,
        detail: "(...args: any[]) => void",
        doc: "Logs a warning message.",
    },
    CompletionEntry {
        label: "error",
        insert_text: "error()",
        kind: CompletionItemKind::METHOD,
        detail: "(...args: any[]) => void",
        doc: "Logs an error message.",
    },
];

/// Provides IDE-style autocomplete suggestions for `environment.`, `request.`, `response.`,
/// `JSON.`, `console.`, and top-level globals in the Post-Script JavaScript editor.
pub(crate) struct PostScriptCompletionProvider {
    env_keys: Rc<RefCell<Vec<String>>>,
}

impl PostScriptCompletionProvider {
    pub(crate) fn new(active_env: Option<&EnvironmentDoc>) -> Self {
        let provider = Self {
            env_keys: Rc::new(RefCell::new(Vec::new())),
        };
        provider.set_env_keys(active_env);
        provider
    }

    pub(crate) fn set_env_keys(&self, active_env: Option<&EnvironmentDoc>) {
        let mut keys: Vec<String> = active_env
            .map(|env| env.variables.iter().map(|v| v.key.clone()).collect())
            .unwrap_or_default();
        keys.sort();
        keys.dedup();
        *self.env_keys.borrow_mut() = keys;
    }

    fn build_items(
        entries: &[CompletionEntry],
        prefix: &str,
        replace_range: Range,
    ) -> Vec<CompletionItem> {
        let lower_prefix = prefix.to_ascii_lowercase();
        entries
            .iter()
            .filter(|e| {
                lower_prefix.is_empty() || e.label.to_ascii_lowercase().starts_with(&lower_prefix)
            })
            .map(|e| CompletionItem {
                label: e.label.to_string(),
                filter_text: Some(prefix.to_string()),
                kind: Some(e.kind),
                detail: Some(e.detail.to_string()),
                documentation: Some(Documentation::String(e.doc.to_string())),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: e.insert_text.to_string(),
                })),
                ..Default::default()
            })
            .collect()
    }
}

fn is_expr_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' || ch == '.'
}

fn is_valid_js_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn extract_expr_before_cursor(rope: &Rope, offset: usize) -> (usize, String) {
    let clamped = offset.min(rope.len());
    let mut start = clamped;
    while start > 0 {
        let Some(ch) = rope.char_at(start - 1) else {
            break;
        };
        if !is_expr_char(ch) {
            break;
        }
        start -= ch.len_utf8();
    }
    let expr = rope.slice(start..clamped).to_string();
    (start, expr)
}

impl CompletionProvider for PostScriptCompletionProvider {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let (expr_start, expr) = extract_expr_before_cursor(rope, offset);
        if expr.is_empty() {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        }

        let items = if let Some(dot_rel) = expr.rfind('.') {
            let receiver = &expr[..dot_rel];
            let member_prefix = &expr[dot_rel + 1..];
            let replace_start = expr_start + dot_rel + 1;
            let replace_range = Range {
                start: rope.offset_to_position(replace_start),
                end: rope.offset_to_position(offset.min(rope.len())),
            };

            match receiver {
                "environment" => {
                    Self::build_items(ENVIRONMENT_COMPLETIONS, member_prefix, replace_range)
                }
                "environment.var" => {
                    let lower_prefix = member_prefix.to_ascii_lowercase();
                    self.env_keys
                        .borrow()
                        .iter()
                        .filter(|k| {
                            is_valid_js_ident(k)
                                && (lower_prefix.is_empty()
                                    || k.to_ascii_lowercase().starts_with(&lower_prefix))
                        })
                        .map(|k| CompletionItem {
                            label: k.clone(),
                            filter_text: Some(member_prefix.to_string()),
                            kind: Some(CompletionItemKind::FIELD),
                            detail: Some("env variable".to_string()),
                            documentation: Some(Documentation::String(format!(
                                "Environment variable `{k}`"
                            ))),
                            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                                range: replace_range,
                                new_text: k.clone(),
                            })),
                            ..Default::default()
                        })
                        .collect()
                }
                "request" => Self::build_items(REQUEST_COMPLETIONS, member_prefix, replace_range),
                "response" => Self::build_items(RESPONSE_COMPLETIONS, member_prefix, replace_range),
                "JSON" => Self::build_items(JSON_COMPLETIONS, member_prefix, replace_range),
                "console" => Self::build_items(CONSOLE_COMPLETIONS, member_prefix, replace_range),
                _ => Vec::new(),
            }
        } else {
            let replace_range = Range {
                start: rope.offset_to_position(expr_start),
                end: rope.offset_to_position(offset.min(rope.len())),
            };
            Self::build_items(TOP_LEVEL_COMPLETIONS, &expr, replace_range)
        };

        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _: &mut App) -> bool {
        new_text == "." || new_text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

#[cfg(test)]
mod tests {
    use testrix_core::{HttpMethod, ResponseHeader, SentRequestSnapshot};

    use super::*;

    #[test]
    fn post_script_parses_response_body_and_sets_environment_var() {
        let mut env = EnvironmentDoc::new("env-dev", "Development");
        env.variables.push(EnvVariable::new("api-token", "old"));

        let body = r#"{"token":"secret-jwt-123","user":{"id":7}}"#;
        let snapshot = HttpResponseSnapshot {
            status_code: 200,
            reason: "OK".to_string(),
            http_version: "HTTP/1.1".to_string(),
            duration_ms: 15,
            size_bytes: body.len(),
            headers: vec![ResponseHeader {
                name: "content-type".to_string(),
                value: "application/json".to_string(),
            }],
            body_bytes: body.as_bytes().to_vec(),
            body_text: body.to_string(),
            pretty_body: body.to_string(),
            content_type: Some("application/json".to_string()),
            sent_request: SentRequestSnapshot {
                method: HttpMethod::Post,
                url: "https://api.example.com/login".to_string(),
                headers: Vec::new(),
                body_preview: String::new(),
            },
        };

        let script = r#"
            const data = JSON.parse(response.body);
            environment.var["api-token"] = data.token;
            environment.var["user-id"] = String(data.user.id);
        "#;

        let result = run_post_script(script, "req-login", "Login", Some(&env), &snapshot)
            .expect("post-script should succeed");

        assert_eq!(
            result.next_vars.get("api-token").map(String::as_str),
            Some("secret-jwt-123")
        );
        assert_eq!(
            result.next_vars.get("user-id").map(String::as_str),
            Some("7")
        );
        assert!(result.changed_keys.contains(&"api-token".to_string()));
        assert!(result.changed_keys.contains(&"user-id".to_string()));

        let applied = apply_vars_to_environment(&mut env, &result.next_vars);
        assert_eq!(applied.len(), 2);
        assert_eq!(env.variables[0].value, "secret-jwt-123");
        assert_eq!(env.variables[1].key, "user-id");
        assert_eq!(env.variables[1].value, "7");
    }

    #[test]
    fn post_script_handles_large_nested_json_payload() {
        let env = EnvironmentDoc::new("env-dev", "Development");
        let roles: Vec<serde_json::Value> = (0..20)
            .map(|i| {
                serde_json::json!({
                    "id": format!("8351787738999029{i:02}"),
                    "roleCode": format!("ROLE_{i}"),
                    "roleName": "科主任",
                    "isInit": true
                })
            })
            .collect();
        let body = serde_json::json!({
            "code": "SUCCESS",
            "data": {
                "dataInfo": {
                    "userInfo": {
                        "deptName": "信息科",
                        "roles": roles,
                    },
                    "tokenRes": {
                        "access_token": "jwt-access-token-xyz",
                        "refresh_token": "jwt-refresh-token-abc",
                        "token_type": "bearer"
                    }
                }
            }
        })
        .to_string();

        let snapshot = HttpResponseSnapshot {
            status_code: 200,
            reason: "OK".to_string(),
            http_version: "HTTP/1.1".to_string(),
            duration_ms: 20,
            size_bytes: body.len(),
            headers: Vec::new(),
            body_bytes: body.as_bytes().to_vec(),
            body_text: body.clone(),
            pretty_body: body,
            content_type: Some("application/json".to_string()),
            sent_request: SentRequestSnapshot {
                method: HttpMethod::Post,
                url: "https://api.example.com/auth".to_string(),
                headers: Vec::new(),
                body_preview: String::new(),
            },
        };

        let script = r#"
            if (response.status >= 200 && response.status < 300) {
              const respBody = JSON.parse(response.body);
              environment.var["api_token"] = respBody.data.dataInfo.tokenRes.access_token;
            }
        "#;

        let result = run_post_script(script, "req-auth", "Auth", Some(&env), &snapshot)
            .expect("post-script should succeed on large nested JSON");
        assert_eq!(
            result.next_vars.get("api_token").map(String::as_str),
            Some("jwt-access-token-xyz")
        );
    }
}
