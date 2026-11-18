use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

use anyhow::{Context as _, Result};
use gpui_shell::{HostModule, HostValue, ShellRuntime};
use serde::Serialize;
use testrix_core::{EnvVariable, EnvironmentDoc, HttpResponseSnapshot};

#[derive(Default)]
struct BridgeState {
    context_json: String,
    committed_vars_json: Option<String>,
}

static RUN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static BRIDGE_STATE: OnceLock<Mutex<BridgeState>> = OnceLock::new();
static MODULE_REGISTERED: OnceLock<std::result::Result<(), String>> = OnceLock::new();

fn ensure_post_script_module() -> Result<()> {
    let res = MODULE_REGISTERED.get_or_init(|| {
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
        .map_err(|err| err.to_string())
    });

    match res {
        Ok(()) => Ok(()),
        Err(msg) => anyhow::bail!("failed to register testrix-post-script module: {msg}"),
    }
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

    ensure_post_script_module()?;
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

    let runtime = ShellRuntime::new_isolated().context("create gpui-shell runtime")?;
    runtime
        .load_application(temp_dir.path(), "main.js")
        .context("execute post-script")?;

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
}
