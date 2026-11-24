use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};
use testrix_core::{
    AuthConfig, EnvVariable, EnvironmentNode, FolderNode, HttpMethod, HttpRequestSpec,
    ProjectNode, RequestNode, RequestSet, RequestSetFolder, RequestSetItem, generate_id,
    parse_request_script,
};
use testrix_http::{
    ShareChannel, collect_project_yaml_files, dispatch_share_future, login_testrix_server,
    revoke_cloudflare_r2_share, revoke_testrix_server_share, upload_to_cloudflare_r2,
    upload_to_testrix_server,
};
use testrix_storage::{
    PersistedShareRecord, ShareSettings, create_environment, create_folder, create_project,
    create_request, delete_environment, delete_folder, delete_project, delete_request,
    init_or_load_config, load_workspace, load_workspace_sets, save_config_share_settings,
    save_config_shared_projects, save_environment, save_folder, save_project, save_request,
    save_workspace_sets, validate_yaml_file,
};

#[derive(Debug, Serialize)]
struct ApiResponse<T: Serialize> {
    ok: bool,
    data: T,
}

#[derive(Debug, Serialize)]
struct ApiError {
    ok: bool,
    error: String,
}

pub fn is_api_resource(arg: &str) -> bool {
    matches!(
        arg,
        "project"
            | "projects"
            | "item"
            | "items"
            | "request"
            | "requests"
            | "folder"
            | "folders"
            | "profile"
            | "profiles"
            | "env"
            | "environment"
            | "environments"
            | "var"
            | "vars"
            | "variable"
            | "variables"
            | "package"
            | "packages"
            | "set"
            | "sets"
            | "share"
            | "shares"
            | "notify"
            | "reload"
    )
}

pub fn execute_cli_api(workspace_dir: &Path, args: &[String]) -> Result<String, String> {
    let parsed = parse_api_args(args)?;
    let value = dispatch_api(workspace_dir, &parsed)?;
    if matches!(
        parsed.action.as_str(),
        "create"
            | "update"
            | "delete"
            | "set"
            | "merge"
            | "add-folder"
            | "update-folder"
            | "delete-folder"
            | "add-item"
            | "remove-item"
    ) && !matches!(parsed.resource.as_str(), "share" | "shares" | "notify" | "reload")
    {
        let _ = crate::ipc::send_reload_notification(Some(workspace_dir));
    }
    serde_json::to_string_pretty(&ApiResponse {
        ok: true,
        data: value,
    })
    .map_err(|e| format!("Failed serializing response: {e}"))
}

pub fn format_api_error(err: &str) -> String {
    serde_json::to_string_pretty(&ApiError {
        ok: false,
        error: err.to_string(),
    })
    .unwrap_or_else(|_| format!(r#"{{"ok":false,"error":"{err}"}}"#))
}

#[derive(Debug)]
struct ParsedApiCmd {
    resource: String,
    action: String,
    flags: std::collections::BTreeMap<String, String>,
    bool_flags: std::collections::BTreeSet<String>,
}

fn parse_api_args(args: &[String]) -> Result<ParsedApiCmd, String> {
    let mut positional = Vec::new();
    let mut flags = std::collections::BTreeMap::new();
    let mut bool_flags = std::collections::BTreeSet::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--json" {
            bool_flags.insert("json".to_string());
            i += 1;
        } else if arg == "--overwrite" {
            bool_flags.insert("overwrite".to_string());
            i += 1;
        } else if let Some(rest) = arg.strip_prefix("--") {
            if let Some((k, v)) = rest.split_once('=') {
                flags.insert(k.to_string(), v.to_string());
                i += 1;
            } else if let Some(next) = args.get(i + 1)
                && !next.starts_with("--")
            {
                flags.insert(rest.to_string(), next.clone());
                i += 2;
            } else {
                bool_flags.insert(rest.to_string());
                i += 1;
            }
        } else {
            positional.push(arg.clone());
            i += 1;
        }
    }

    let resource = positional
        .first()
        .ok_or_else(|| "Missing resource (project | item | folder | profile | var)".to_string())?
        .to_ascii_lowercase();
    let action = match positional.get(1) {
        Some(a) => a.to_ascii_lowercase(),
        None if resource == "reload" => "reload".to_string(),
        None => return Err(format!("Missing action for `{resource}`")),
    };

    Ok(ParsedApiCmd {
        resource,
        action,
        flags,
        bool_flags,
    })
}

fn req_flag<'a>(cmd: &'a ParsedApiCmd, key: &str) -> Result<&'a str, String> {
    cmd.flags
        .get(key)
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("Missing required flag `--{key}`"))
}

fn opt_flag<'a>(cmd: &'a ParsedApiCmd, key: &str) -> Option<&'a str> {
    cmd.flags.get(key).map(String::as_str)
}

fn parse_bool_str(val: &str, flag: &str) -> Result<bool, String> {
    match val.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(format!(
            "Invalid boolean value `{val}` for `--{flag}` (expected true or false)"
        )),
    }
}

fn validate_non_empty_name(name: &str, kind: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(format!("{kind} name must not be empty"));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(format!("{kind} name must not contain control characters"));
    }
    Ok(trimmed.to_string())
}

fn validate_base_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("baseUrl must not contain control characters".to_string());
    }
    if !trimmed.starts_with("{{")
        && !trimmed.starts_with("http://")
        && !trimmed.starts_with("https://")
    {
        return Err(
            "baseUrl must start with `http://`, `https://`, or a template variable `{{...}}`"
                .to_string(),
        );
    }
    Ok(trimmed.to_string())
}

fn validate_request_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("Request URL must not contain control characters".to_string());
    }
    Ok(trimmed.to_string())
}

fn validate_variable_key(key: &str) -> Result<String, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("Variable key must not be empty".to_string());
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "Variable key `{trimmed}` must not contain whitespace or control characters"
        ));
    }
    if trimmed.contains("{{") || trimmed.contains("}}") || trimmed.contains('=') {
        return Err(format!(
            "Variable key `{trimmed}` contains illegal characters (`{{{{`, `}}}}`, or `=`)"
        ));
    }
    Ok(trimmed.to_string())
}

fn find_project<'a>(
    projects: &'a [ProjectNode],
    id_or_name: &str,
) -> Result<&'a ProjectNode, String> {
    projects
        .iter()
        .find(|p| p.doc.id == id_or_name || p.doc.name == id_or_name)
        .ok_or_else(|| format!("Project `{id_or_name}` not found"))
}

fn find_project_mut<'a>(
    projects: &'a mut [ProjectNode],
    id_or_name: &str,
) -> Result<&'a mut ProjectNode, String> {
    projects
        .iter_mut()
        .find(|p| p.doc.id == id_or_name || p.doc.name == id_or_name)
        .ok_or_else(|| format!("Project `{id_or_name}` not found"))
}

fn find_folder_in_project<'a>(
    proj: &'a ProjectNode,
    id_or_name: &str,
) -> Result<&'a FolderNode, String> {
    fn walk<'a>(folders: &'a [FolderNode], target: &str) -> Option<&'a FolderNode> {
        for f in folders {
            if f.doc.id == target || f.doc.name == target {
                return Some(f);
            }
            if let Some(sub) = walk(&f.folders, target) {
                return Some(sub);
            }
        }
        None
    }
    walk(&proj.folders, id_or_name).ok_or_else(|| format!("Folder `{id_or_name}` not found"))
}

fn find_folder_mut_in_project<'a>(
    proj: &'a mut ProjectNode,
    id_or_name: &str,
) -> Result<&'a mut FolderNode, String> {
    fn walk<'a>(folders: &'a mut [FolderNode], target: &str) -> Option<&'a mut FolderNode> {
        for f in folders {
            if f.doc.id == target || f.doc.name == target {
                return Some(f);
            }
            if let Some(sub) = walk(&mut f.folders, target) {
                return Some(sub);
            }
        }
        None
    }
    walk(&mut proj.folders, id_or_name).ok_or_else(|| format!("Folder `{id_or_name}` not found"))
}

fn find_request_in_project<'a>(
    proj: &'a ProjectNode,
    id_or_name: &str,
) -> Result<&'a RequestNode, String> {
    fn walk<'a>(folders: &'a [FolderNode], target: &str) -> Option<&'a RequestNode> {
        for f in folders {
            if let Some(r) = f
                .requests
                .iter()
                .find(|r| r.doc.id == target || r.doc.name == target)
            {
                return Some(r);
            }
            if let Some(r) = walk(&f.folders, target) {
                return Some(r);
            }
        }
        None
    }
    proj.requests
        .iter()
        .find(|r| r.doc.id == id_or_name || r.doc.name == id_or_name)
        .or_else(|| walk(&proj.folders, id_or_name))
        .ok_or_else(|| format!("Request item `{id_or_name}` not found"))
}

fn find_request_mut_in_project<'a>(
    proj: &'a mut ProjectNode,
    id_or_name: &str,
) -> Result<&'a mut RequestNode, String> {
    fn walk<'a>(folders: &'a mut [FolderNode], target: &str) -> Option<&'a mut RequestNode> {
        for f in folders {
            if let Some(r) = f
                .requests
                .iter_mut()
                .find(|r| r.doc.id == target || r.doc.name == target)
            {
                return Some(r);
            }
            if let Some(r) = walk(&mut f.folders, target) {
                return Some(r);
            }
        }
        None
    }
    if let Some(r) = proj
        .requests
        .iter_mut()
        .find(|r| r.doc.id == id_or_name || r.doc.name == id_or_name)
    {
        return Ok(r);
    }
    walk(&mut proj.folders, id_or_name)
        .ok_or_else(|| format!("Request item `{id_or_name}` not found"))
}

fn find_env<'a>(envs: &'a [EnvironmentNode], id_or_name: &str) -> Result<&'a EnvironmentNode, String> {
    envs.iter()
        .find(|e| e.doc.id == id_or_name || e.doc.name == id_or_name)
        .ok_or_else(|| format!("Profile `{id_or_name}` not found"))
}

fn find_env_mut<'a>(
    envs: &'a mut [EnvironmentNode],
    id_or_name: &str,
) -> Result<&'a mut EnvironmentNode, String> {
    envs.iter_mut()
        .find(|e| e.doc.id == id_or_name || e.doc.name == id_or_name)
        .ok_or_else(|| format!("Profile `{id_or_name}` not found"))
}

fn dispatch_api(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    match cmd.resource.as_str() {
        "project" | "projects" => handle_project(workspace_dir, cmd),
        "folder" | "folders" => handle_folder(workspace_dir, cmd),
        "item" | "items" | "request" | "requests" => handle_item(workspace_dir, cmd),
        "profile" | "profiles" | "env" | "environment" | "environments" => {
            handle_profile(workspace_dir, cmd)
        }
        "var" | "vars" | "variable" | "variables" => handle_var(workspace_dir, cmd),
        "package" | "packages" | "set" | "sets" => handle_package(workspace_dir, cmd),
        "share" | "shares" => handle_share(workspace_dir, cmd),
        "notify" | "reload" => handle_notify(workspace_dir, cmd),
        other => Err(format!("Unknown resource `{other}`")),
    }
}

fn handle_notify(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    match cmd.action.as_str() {
        "reload" => match crate::ipc::send_reload_notification(Some(workspace_dir)) {
            Ok(resp) => Ok(json!({
                "notified": true,
                "reloaded": resp.reloaded,
                "transport": crate::ipc::ipc_transport_name(),
                "message": resp.message,
            })),
            Err(reason) => Ok(json!({
                "notified": false,
                "reloaded": false,
                "transport": crate::ipc::ipc_transport_name(),
                "reason": reason,
            })),
        },
        other => Err(format!("Unsupported notify action `{other}` (expected `reload`)")),
    }
}

fn handle_project(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    match cmd.action.as_str() {
        "list" => {
            let tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
            let list: Vec<Value> = tree
                .projects
                .iter()
                .map(|p| {
                    json!({
                        "id": p.doc.id,
                        "name": p.doc.name,
                        "baseUrl": p.doc.base_url,
                        "description": p.doc.description,
                        "requestCount": p.request_count(),
                        "profileCount": p.environments.len(),
                    })
                })
                .collect();
            Ok(json!(list))
        }
        "get" => {
            let target = req_flag(cmd, "project")?;
            let tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
            let proj = find_project(&tree.projects, target)?;
            serde_json::to_value(&proj.doc).map_err(|e| e.to_string())
        }
        "create" => {
            let name = validate_non_empty_name(req_flag(cmd, "name")?, "Project")?;
            let base_url = validate_base_url(opt_flag(cmd, "base-url").unwrap_or(""))?;
            let mut proj = create_project(workspace_dir, &name, &base_url)
                .map_err(|e| e.to_string())?;
            if let Some(desc) = opt_flag(cmd, "description") {
                proj.doc.description = desc.to_string();
                proj.doc.validate().map_err(|e| e.to_string())?;
                save_project(&proj).map_err(|e| e.to_string())?;
            }
            validate_yaml_file(&proj.manifest_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&proj.doc).map_err(|e| e.to_string())
        }
        "update" => {
            let target = req_flag(cmd, "project")?;
            let mut tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
            let proj = find_project_mut(&mut tree.projects, target)?;
            if let Some(name) = opt_flag(cmd, "name") {
                proj.doc.name = validate_non_empty_name(name, "Project")?;
            }
            if let Some(base_url) = opt_flag(cmd, "base-url") {
                proj.doc.base_url = validate_base_url(base_url)?;
            }
            if let Some(desc) = opt_flag(cmd, "description") {
                proj.doc.description = desc.to_string();
            }
            if let Some(auth_json) = opt_flag(cmd, "auth-json") {
                let auth: AuthConfig = serde_json::from_str(auth_json)
                    .map_err(|e| format!("Invalid `--auth-json`: {e}"))?;
                proj.doc.auth = auth;
            }
            proj.doc.validate().map_err(|e| e.to_string())?;
            save_project(proj).map_err(|e| e.to_string())?;
            validate_yaml_file(&proj.manifest_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&proj.doc).map_err(|e| e.to_string())
        }
        "delete" => {
            let target = req_flag(cmd, "project")?;
            let tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
            let proj = find_project(&tree.projects, target)?;
            let id = proj.doc.id.clone();
            delete_project(&proj.root_dir).map_err(|e| e.to_string())?;
            Ok(json!({ "deleted": id }))
        }
        other => Err(format!("Unknown project action `{other}`")),
    }
}

fn handle_folder(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    let project_ref = req_flag(cmd, "project")?;
    let mut tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
    let proj = find_project_mut(&mut tree.projects, project_ref)?;

    match cmd.action.as_str() {
        "list" => {
            fn collect(folders: &[FolderNode], out: &mut Vec<Value>) {
                for f in folders {
                    out.push(json!({
                        "id": f.doc.id,
                        "name": f.doc.name,
                        "order": f.doc.order,
                        "requestCount": f.requests.len(),
                    }));
                    collect(&f.folders, out);
                }
            }
            let mut out = Vec::new();
            collect(&proj.folders, &mut out);
            Ok(json!(out))
        }
        "create" => {
            let name = validate_non_empty_name(req_flag(cmd, "name")?, "Folder")?;
            let order = opt_flag(cmd, "order")
                .map(|v| {
                    v.trim()
                        .parse::<i32>()
                        .map_err(|e| format!("Invalid `--order`: {e}"))
                })
                .transpose()?
                .unwrap_or(0);
            let parent_dir = match opt_flag(cmd, "parent") {
                Some(p) => find_folder_in_project(proj, p)?.dir_path.clone(),
                None => proj.root_dir.clone(),
            };
            let mut folder = create_folder(&parent_dir, &name, order).map_err(|e| e.to_string())?;
            if let Some(desc) = opt_flag(cmd, "description") {
                folder.doc.description = desc.to_string();
                folder.doc.validate().map_err(|e| e.to_string())?;
                save_folder(&folder).map_err(|e| e.to_string())?;
            }
            validate_yaml_file(&folder.manifest_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&folder.doc).map_err(|e| e.to_string())
        }
        "update" => {
            let folder_ref = req_flag(cmd, "folder")?;
            let folder = find_folder_mut_in_project(proj, folder_ref)?;
            if let Some(name) = opt_flag(cmd, "name") {
                folder.doc.name = validate_non_empty_name(name, "Folder")?;
            }
            if let Some(desc) = opt_flag(cmd, "description") {
                folder.doc.description = desc.to_string();
            }
            if let Some(order_str) = opt_flag(cmd, "order") {
                folder.doc.order = order_str
                    .trim()
                    .parse::<i32>()
                    .map_err(|e| format!("Invalid `--order`: {e}"))?;
            }
            if let Some(auth_json) = opt_flag(cmd, "auth-json") {
                let auth: AuthConfig = serde_json::from_str(auth_json)
                    .map_err(|e| format!("Invalid `--auth-json`: {e}"))?;
                folder.doc.auth = auth;
            }
            folder.doc.validate().map_err(|e| e.to_string())?;
            save_folder(folder).map_err(|e| e.to_string())?;
            validate_yaml_file(&folder.manifest_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&folder.doc).map_err(|e| e.to_string())
        }
        "delete" => {
            let folder_ref = req_flag(cmd, "folder")?;
            let folder = find_folder_in_project(proj, folder_ref)?;
            let id = folder.doc.id.clone();
            delete_folder(&folder.dir_path).map_err(|e| e.to_string())?;
            Ok(json!({ "deleted": id }))
        }
        other => Err(format!("Unknown folder action `{other}`")),
    }
}

fn handle_item(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    let project_ref = req_flag(cmd, "project")?;
    let mut tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
    let proj = find_project_mut(&mut tree.projects, project_ref)?;

    match cmd.action.as_str() {
        "list" => {
            fn collect_reqs(reqs: &[RequestNode], out: &mut Vec<Value>) {
                for r in reqs {
                    out.push(json!({
                        "id": r.doc.id,
                        "name": r.doc.name,
                        "method": r.doc.request.method.as_str(),
                        "url": r.doc.request.url,
                    }));
                }
            }
            fn walk_folders(folders: &[FolderNode], out: &mut Vec<Value>) {
                for f in folders {
                    collect_reqs(&f.requests, out);
                    walk_folders(&f.folders, out);
                }
            }
            let mut out = Vec::new();
            if let Some(folder_ref) = opt_flag(cmd, "folder") {
                let folder = find_folder_in_project(proj, folder_ref)?;
                collect_reqs(&folder.requests, &mut out);
            } else {
                collect_reqs(&proj.requests, &mut out);
                walk_folders(&proj.folders, &mut out);
            }
            Ok(json!(out))
        }
        "get" => {
            let item_ref = req_flag(cmd, "item")?;
            let req = find_request_in_project(proj, item_ref)?;
            serde_json::to_value(&req.doc).map_err(|e| e.to_string())
        }
        "create" => {
            let name = validate_non_empty_name(req_flag(cmd, "name")?, "Request")?;
            let parent_dir = match opt_flag(cmd, "folder") {
                Some(f) => find_folder_in_project(proj, f)?.dir_path.clone(),
                None => proj.root_dir.clone(),
            };

            let spec = if let Some(spec_json) = opt_flag(cmd, "spec-json") {
                serde_json::from_str::<HttpRequestSpec>(spec_json)
                    .map_err(|e| format!("Invalid `--spec-json`: {e}"))?
            } else if let Some(script) = opt_flag(cmd, "script") {
                parse_request_script(script).map_err(|e| e.to_string())?
            } else {
                let method = match opt_flag(cmd, "method") {
                    Some(m) => HttpMethod::parse(m)
                        .ok_or_else(|| format!("Unsupported HTTP method `{m}`"))?,
                    None => HttpMethod::Get,
                };
                let url = validate_request_url(opt_flag(cmd, "url").unwrap_or(""))?;
                let mut spec = HttpRequestSpec::default();
                spec.method = method;
                spec.url = url;
                spec
            };

            let mut req = create_request(&parent_dir, &name, spec.method, &spec.url)
                .map_err(|e| e.to_string())?;
            req.doc.request = spec;
            if let Some(desc) = opt_flag(cmd, "description") {
                req.doc.description = desc.to_string();
            }
            req.doc.validate().map_err(|e| e.to_string())?;
            save_request(&req).map_err(|e| e.to_string())?;
            validate_yaml_file(&req.file_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&req.doc).map_err(|e| e.to_string())
        }
        "update" => {
            let item_ref = req_flag(cmd, "item")?;
            let req = find_request_mut_in_project(proj, item_ref)?;

            if let Some(spec_json) = opt_flag(cmd, "spec-json") {
                req.doc.request = serde_json::from_str::<HttpRequestSpec>(spec_json)
                    .map_err(|e| format!("Invalid `--spec-json`: {e}"))?;
            } else if let Some(script) = opt_flag(cmd, "script") {
                req.doc.request = parse_request_script(script).map_err(|e| e.to_string())?;
            }
            if let Some(name) = opt_flag(cmd, "name") {
                req.doc.name = validate_non_empty_name(name, "Request")?;
            }
            if let Some(desc) = opt_flag(cmd, "description") {
                req.doc.description = desc.to_string();
            }
            if let Some(m) = opt_flag(cmd, "method") {
                req.doc.request.method = HttpMethod::parse(m)
                    .ok_or_else(|| format!("Unsupported HTTP method `{m}`"))?;
            }
            if let Some(url) = opt_flag(cmd, "url") {
                req.doc.request.url = validate_request_url(url)?;
            }

            req.doc.validate().map_err(|e| e.to_string())?;
            save_request(req).map_err(|e| e.to_string())?;
            validate_yaml_file(&req.file_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&req.doc).map_err(|e| e.to_string())
        }
        "delete" => {
            let item_ref = req_flag(cmd, "item")?;
            let req = find_request_in_project(proj, item_ref)?;
            let id = req.doc.id.clone();
            delete_request(&req.file_path).map_err(|e| e.to_string())?;
            Ok(json!({ "deleted": id }))
        }
        other => Err(format!("Unknown item action `{other}`")),
    }
}

fn handle_profile(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    let mut tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;

    match cmd.action.as_str() {
        "list" => {
            let list: Vec<Value> = tree
                .environments
                .iter()
                .map(|e| {
                    json!({
                        "id": e.doc.id,
                        "name": e.doc.name,
                        "description": e.doc.description,
                        "variableCount": e.doc.variables.len(),
                    })
                })
                .collect();
            Ok(json!(list))
        }
        "get" => {
            let prof_ref = req_flag(cmd, "profile")?;
            let env = find_env(&tree.environments, prof_ref)?;
            serde_json::to_value(&env.doc).map_err(|e| e.to_string())
        }
        "create" => {
            let name = validate_non_empty_name(req_flag(cmd, "name")?, "Profile")?;
            if tree
                .environments
                .iter()
                .any(|e| e.doc.name.eq_ignore_ascii_case(&name))
            {
                return Err(format!("Profile `{name}` already exists in workspace"));
            }
            let mut env = create_environment(workspace_dir, &name).map_err(|e| e.to_string())?;
            if let Some(desc) = opt_flag(cmd, "description") {
                env.doc.description = desc.to_string();
                env.doc.validate().map_err(|e| e.to_string())?;
                save_environment(&env).map_err(|e| e.to_string())?;
            }
            validate_yaml_file(&env.file_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&env.doc).map_err(|e| e.to_string())
        }
        "update" => {
            let prof_ref = req_flag(cmd, "profile")?;
            let env = find_env_mut(&mut tree.environments, prof_ref)?;
            if let Some(name) = opt_flag(cmd, "name") {
                env.doc.name = validate_non_empty_name(name, "Profile")?;
            }
            if let Some(desc) = opt_flag(cmd, "description") {
                env.doc.description = desc.to_string();
            }
            env.doc.validate().map_err(|e| e.to_string())?;
            save_environment(env).map_err(|e| e.to_string())?;
            validate_yaml_file(&env.file_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&env.doc).map_err(|e| e.to_string())
        }
        "delete" => {
            let prof_ref = req_flag(cmd, "profile")?;
            let env = find_env(&tree.environments, prof_ref)?;
            let id = env.doc.id.clone();
            delete_environment(&env.file_path).map_err(|e| e.to_string())?;
            Ok(json!({ "deleted": id }))
        }
        "merge" => {
            let target_ref = req_flag(cmd, "profile")?;
            let source_ref = req_flag(cmd, "from")?;
            let overwrite = cmd.bool_flags.contains("overwrite");

            let source_vars = {
                let src = find_env(&tree.environments, source_ref)?;
                if src.doc.id == find_env(&tree.environments, target_ref)?.doc.id {
                    return Err("Cannot merge a profile into itself".to_string());
                }
                src.doc.variables.clone()
            };

            let target = find_env_mut(&mut tree.environments, target_ref)?;
            let mut added = 0usize;
            let mut updated = 0usize;
            for sv in source_vars {
                if let Some(existing) = target.doc.variables.iter_mut().find(|v| v.key == sv.key) {
                    if overwrite {
                        *existing = sv;
                        updated += 1;
                    }
                } else {
                    target.doc.variables.push(sv);
                    added += 1;
                }
            }
            target.doc.validate().map_err(|e| e.to_string())?;
            save_environment(target).map_err(|e| e.to_string())?;
            validate_yaml_file(&target.file_path).map_err(|e| e.to_string())?;
            Ok(json!({
                "profile": target.doc.id,
                "added": added,
                "updated": updated,
            }))
        }
        other => Err(format!("Unknown profile action `{other}`")),
    }
}

fn handle_var(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    let prof_ref = req_flag(cmd, "profile")?;
    let mut tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
    let env = find_env_mut(&mut tree.environments, prof_ref)?;

    match cmd.action.as_str() {
        "list" => serde_json::to_value(&env.doc.variables).map_err(|e| e.to_string()),
        "get" => {
            let key = validate_variable_key(req_flag(cmd, "key")?)?;
            let var = env
                .doc
                .variables
                .iter()
                .find(|v| v.key == key)
                .ok_or_else(|| format!("Variable `{key}` not found"))?;
            serde_json::to_value(var).map_err(|e| e.to_string())
        }
        "create" => {
            let key = validate_variable_key(req_flag(cmd, "key")?)?;
            if env.doc.variables.iter().any(|v| v.key == key) {
                return Err(format!("Variable `{key}` already exists in profile"));
            }
            let value = opt_flag(cmd, "value").unwrap_or("").to_string();
            let mut var = EnvVariable::new(&key, value);
            if let Some(s) = opt_flag(cmd, "secret") {
                var.secret = parse_bool_str(s, "secret")?;
            }
            if let Some(e) = opt_flag(cmd, "enabled") {
                var.enabled = parse_bool_str(e, "enabled")?;
            }
            if let Some(d) = opt_flag(cmd, "description") {
                var.description = d.to_string();
            }
            env.doc.variables.push(var.clone());
            env.doc.validate().map_err(|e| e.to_string())?;
            save_environment(env).map_err(|e| e.to_string())?;
            validate_yaml_file(&env.file_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&var).map_err(|e| e.to_string())
        }
        "update" => {
            let key = validate_variable_key(req_flag(cmd, "key")?)?;
            let new_key = opt_flag(cmd, "new-key")
                .map(validate_variable_key)
                .transpose()?;
            if let Some(ref nk) = new_key
                && nk != &key
                && env.doc.variables.iter().any(|v| &v.key == nk)
            {
                return Err(format!("Target variable key `{nk}` already exists"));
            }
            let next_secret = opt_flag(cmd, "secret")
                .map(|s| parse_bool_str(s, "secret"))
                .transpose()?;
            let next_enabled = opt_flag(cmd, "enabled")
                .map(|e| parse_bool_str(e, "enabled"))
                .transpose()?;

            let var = env
                .doc
                .variables
                .iter_mut()
                .find(|v| v.key == key)
                .ok_or_else(|| format!("Variable `{key}` not found"))?;
            if let Some(nk) = new_key {
                var.key = nk;
            }
            if let Some(val) = opt_flag(cmd, "value") {
                var.value = val.to_string();
            }
            if let Some(s) = next_secret {
                var.secret = s;
            }
            if let Some(e) = next_enabled {
                var.enabled = e;
            }
            if let Some(d) = opt_flag(cmd, "description") {
                var.description = d.to_string();
            }
            let snapshot = var.clone();
            env.doc.validate().map_err(|e| e.to_string())?;
            save_environment(env).map_err(|e| e.to_string())?;
            validate_yaml_file(&env.file_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&snapshot).map_err(|e| e.to_string())
        }
        "set" => {
            let key = validate_variable_key(req_flag(cmd, "key")?)?;
            let next_secret = opt_flag(cmd, "secret")
                .map(|s| parse_bool_str(s, "secret"))
                .transpose()?;
            let next_enabled = opt_flag(cmd, "enabled")
                .map(|e| parse_bool_str(e, "enabled"))
                .transpose()?;

            let snapshot = if let Some(var) = env.doc.variables.iter_mut().find(|v| v.key == key) {
                if let Some(val) = opt_flag(cmd, "value") {
                    var.value = val.to_string();
                }
                if let Some(s) = next_secret {
                    var.secret = s;
                }
                if let Some(e) = next_enabled {
                    var.enabled = e;
                }
                if let Some(d) = opt_flag(cmd, "description") {
                    var.description = d.to_string();
                }
                var.clone()
            } else {
                let mut var = EnvVariable::new(&key, opt_flag(cmd, "value").unwrap_or(""));
                if let Some(s) = next_secret {
                    var.secret = s;
                }
                if let Some(e) = next_enabled {
                    var.enabled = e;
                }
                if let Some(d) = opt_flag(cmd, "description") {
                    var.description = d.to_string();
                }
                env.doc.variables.push(var.clone());
                var
            };
            env.doc.validate().map_err(|e| e.to_string())?;
            save_environment(env).map_err(|e| e.to_string())?;
            validate_yaml_file(&env.file_path).map_err(|e| e.to_string())?;
            serde_json::to_value(&snapshot).map_err(|e| e.to_string())
        }
        "delete" => {
            let key = validate_variable_key(req_flag(cmd, "key")?)?;
            let before = env.doc.variables.len();
            env.doc.variables.retain(|v| v.key != key);
            if env.doc.variables.len() == before {
                return Err(format!("Variable `{key}` not found"));
            }
            env.doc.validate().map_err(|e| e.to_string())?;
            save_environment(env).map_err(|e| e.to_string())?;
            validate_yaml_file(&env.file_path).map_err(|e| e.to_string())?;
            Ok(json!({ "deleted": key }))
        }
        other => Err(format!("Unknown var action `{other}`")),
    }
}

fn parse_share_channel(raw: &str) -> Result<ShareChannel, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "server" | "testrix" | "testrix-server" | "testrix_server" | "testrix share server" => {
            Ok(ShareChannel::TestrixServer)
        }
        "r2" | "cloudflare" | "cloudflare-r2" | "cloudflare_r2" | "cloudflare r2 storage" => {
            Ok(ShareChannel::CloudflareR2)
        }
        "local" | "local-network" | "local_network" | "local network" => Err(
            "Local Network sharing runs inside the GUI desktop app; use `server` or `r2` in CLI mode."
                .to_string(),
        ),
        _ => ShareChannel::from_label(raw)
            .filter(|ch| *ch != ShareChannel::LocalNetwork)
            .ok_or_else(|| {
                format!("Unsupported share channel `{raw}` (expected `server` or `r2`)")
            }),
    }
}

fn project_slug_from_node(project: &ProjectNode) -> String {
    project
        .root_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| project.doc.id.clone())
}

fn resolve_default_share_channel(settings: &ShareSettings) -> ShareChannel {
    if !settings.share_token.trim().is_empty() {
        ShareChannel::TestrixServer
    } else if !settings.cloudflare_r2_bucket.trim().is_empty()
        && !settings.cloudflare_api_token.trim().is_empty()
    {
        ShareChannel::CloudflareR2
    } else {
        ShareChannel::TestrixServer
    }
}

fn count_set_requests(folders: &[RequestSetFolder], requests: &[RequestSetItem]) -> usize {
    requests.len()
        + folders
            .iter()
            .map(|f| count_set_requests(&f.folders, &f.requests))
            .sum::<usize>()
}

fn find_set<'a>(sets: &'a [RequestSet], id_or_name: &str) -> Result<&'a RequestSet, String> {
    sets.iter()
        .find(|s| s.id == id_or_name || s.name == id_or_name)
        .ok_or_else(|| format!("Package `{id_or_name}` not found"))
}

fn find_set_mut<'a>(
    sets: &'a mut [RequestSet],
    id_or_name: &str,
) -> Result<&'a mut RequestSet, String> {
    sets.iter_mut()
        .find(|s| s.id == id_or_name || s.name == id_or_name)
        .ok_or_else(|| format!("Package `{id_or_name}` not found"))
}

fn find_set_folder_mut<'a>(
    folders: &'a mut [RequestSetFolder],
    id_or_name: &str,
) -> Result<&'a mut RequestSetFolder, String> {
    for folder in folders {
        if folder.id == id_or_name || folder.name == id_or_name {
            return Ok(folder);
        }
        if let Ok(found) = find_set_folder_mut(&mut folder.folders, id_or_name) {
            return Ok(found);
        }
    }
    Err(format!("Package folder `{id_or_name}` not found"))
}

fn remove_set_folder(folders: &mut Vec<RequestSetFolder>, id_or_name: &str) -> Option<String> {
    if let Some(pos) = folders
        .iter()
        .position(|f| f.id == id_or_name || f.name == id_or_name)
    {
        return Some(folders.remove(pos).id);
    }
    for folder in folders {
        if let Some(removed) = remove_set_folder(&mut folder.folders, id_or_name) {
            return Some(removed);
        }
    }
    None
}

fn remove_set_item(requests: &mut Vec<RequestSetItem>, item_ref: &str) -> Option<String> {
    let pos = requests
        .iter()
        .position(|r| r.id == item_ref || r.request_id == item_ref || r.name == item_ref)?;
    Some(requests.remove(pos).id)
}

fn handle_package(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    let mut sets = load_workspace_sets(workspace_dir);

    match cmd.action.as_str() {
        "list" => {
            let out: Vec<Value> = sets
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "name": s.name,
                        "folderCount": s.folders.len(),
                        "requestCount": s.requests.len(),
                        "totalRequestCount": count_set_requests(&s.folders, &s.requests),
                    })
                })
                .collect();
            Ok(json!(out))
        }
        "get" => {
            let pkg_ref = req_flag(cmd, "package")?;
            let pkg = find_set(&sets, pkg_ref)?;
            serde_json::to_value(pkg).map_err(|e| e.to_string())
        }
        "create" => {
            let name = validate_non_empty_name(req_flag(cmd, "name")?, "Package")?;
            let pkg = RequestSet {
                id: generate_id("set"),
                name,
                folders: Vec::new(),
                requests: Vec::new(),
            };
            sets.push(pkg.clone());
            save_workspace_sets(workspace_dir, &sets).map_err(|e| e.to_string())?;
            serde_json::to_value(&pkg).map_err(|e| e.to_string())
        }
        "update" => {
            let pkg_ref = req_flag(cmd, "package")?;
            let name = validate_non_empty_name(req_flag(cmd, "name")?, "Package")?;
            let pkg = find_set_mut(&mut sets, pkg_ref)?;
            pkg.name = name;
            let updated = pkg.clone();
            save_workspace_sets(workspace_dir, &sets).map_err(|e| e.to_string())?;
            serde_json::to_value(&updated).map_err(|e| e.to_string())
        }
        "delete" => {
            let pkg_ref = req_flag(cmd, "package")?;
            let pos = sets
                .iter()
                .position(|s| s.id == pkg_ref || s.name == pkg_ref)
                .ok_or_else(|| format!("Package `{pkg_ref}` not found"))?;
            let removed = sets.remove(pos);
            save_workspace_sets(workspace_dir, &sets).map_err(|e| e.to_string())?;
            Ok(json!({ "deleted": removed.id }))
        }
        "add-folder" => {
            let pkg_ref = req_flag(cmd, "package")?;
            let name = validate_non_empty_name(req_flag(cmd, "name")?, "Package folder")?;
            let parent_ref = opt_flag(cmd, "parent").map(str::to_string);
            let pkg = find_set_mut(&mut sets, pkg_ref)?;
            let folder = RequestSetFolder {
                id: generate_id("setf"),
                name,
                folders: Vec::new(),
                requests: Vec::new(),
            };
            if let Some(parent) = parent_ref.as_deref() {
                let target = find_set_folder_mut(&mut pkg.folders, parent)?;
                target.folders.push(folder.clone());
            } else {
                pkg.folders.push(folder.clone());
            }
            save_workspace_sets(workspace_dir, &sets).map_err(|e| e.to_string())?;
            serde_json::to_value(&folder).map_err(|e| e.to_string())
        }
        "update-folder" => {
            let pkg_ref = req_flag(cmd, "package")?;
            let folder_ref = req_flag(cmd, "folder")?;
            let name = validate_non_empty_name(req_flag(cmd, "name")?, "Package folder")?;
            let pkg = find_set_mut(&mut sets, pkg_ref)?;
            let folder = find_set_folder_mut(&mut pkg.folders, folder_ref)?;
            folder.name = name;
            let updated = folder.clone();
            save_workspace_sets(workspace_dir, &sets).map_err(|e| e.to_string())?;
            serde_json::to_value(&updated).map_err(|e| e.to_string())
        }
        "delete-folder" => {
            let pkg_ref = req_flag(cmd, "package")?;
            let folder_ref = req_flag(cmd, "folder")?;
            let pkg = find_set_mut(&mut sets, pkg_ref)?;
            let removed_id = remove_set_folder(&mut pkg.folders, folder_ref)
                .ok_or_else(|| format!("Package folder `{folder_ref}` not found"))?;
            save_workspace_sets(workspace_dir, &sets).map_err(|e| e.to_string())?;
            Ok(json!({ "deleted": removed_id }))
        }
        "add-item" => {
            let pkg_ref = req_flag(cmd, "package")?;
            let project_ref = req_flag(cmd, "project")?;
            let item_ref = req_flag(cmd, "item")?;
            let folder_ref = opt_flag(cmd, "folder").map(str::to_string);

            let tree = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
            let proj = find_project(&tree.projects, project_ref)?;
            let req = find_request_in_project(proj, item_ref)?;
            let set_item = RequestSetItem {
                id: generate_id("seti"),
                project_id: proj.doc.id.clone(),
                request_id: req.doc.id.clone(),
                name: req.doc.name.clone(),
                method: req.doc.request.method,
                url: req.doc.request.url.clone(),
            };

            let pkg = find_set_mut(&mut sets, pkg_ref)?;
            if let Some(folder_name) = folder_ref.as_deref() {
                let target = find_set_folder_mut(&mut pkg.folders, folder_name)?;
                target.requests.push(set_item.clone());
            } else {
                pkg.requests.push(set_item.clone());
            }
            save_workspace_sets(workspace_dir, &sets).map_err(|e| e.to_string())?;
            serde_json::to_value(&set_item).map_err(|e| e.to_string())
        }
        "remove-item" => {
            let pkg_ref = req_flag(cmd, "package")?;
            let item_ref = req_flag(cmd, "item")?;
            let folder_ref = opt_flag(cmd, "folder").map(str::to_string);
            let pkg = find_set_mut(&mut sets, pkg_ref)?;
            let removed_id = if let Some(folder_name) = folder_ref.as_deref() {
                let target = find_set_folder_mut(&mut pkg.folders, folder_name)?;
                remove_set_item(&mut target.requests, item_ref)
            } else {
                remove_set_item(&mut pkg.requests, item_ref)
            }
            .ok_or_else(|| format!("Package item `{item_ref}` not found"))?;
            save_workspace_sets(workspace_dir, &sets).map_err(|e| e.to_string())?;
            Ok(json!({ "deleted": removed_id }))
        }
        other => Err(format!("Unknown package action `{other}`")),
    }
}

fn handle_share(workspace_dir: &Path, cmd: &ParsedApiCmd) -> Result<Value, String> {
    match cmd.action.as_str() {
        "list" => {
            let cfg = init_or_load_config().map_err(|e| e.to_string())?;
            let project_filter = opt_flag(cmd, "project").map(str::trim);
            let channel_filter = opt_flag(cmd, "channel")
                .map(parse_share_channel)
                .transpose()?;
            let records: Vec<Value> = cfg
                .shared_projects
                .iter()
                .filter(|r| {
                    project_filter.is_none_or(|f| {
                        r.project_id == f || r.project_name == f || r.project_slug == f
                    })
                })
                .filter(|r| {
                    channel_filter
                        .is_none_or(|ch| ShareChannel::from_label(&r.channel) == Some(ch))
                })
                .map(|r| {
                    json!({
                        "id": r.id,
                        "project_id": r.project_id,
                        "project_name": r.project_name,
                        "project_slug": r.project_slug,
                        "channel": r.channel,
                        "share_url": r.share_url,
                        "target_info": r.target_info,
                    })
                })
                .collect();
            Ok(json!(records))
        }
        "status" | "config" => {
            let cfg = init_or_load_config().map_err(|e| e.to_string())?;
            Ok(json!({
                "share_server_url": cfg.share_server_url,
                "share_email": cfg.share_email,
                "logged_in": !cfg.share_token.trim().is_empty(),
                "cloudflare_account_id": cfg.cloudflare_account_id,
                "cloudflare_r2_bucket": cfg.cloudflare_r2_bucket,
                "cloudflare_pages_url": cfg.cloudflare_pages_url,
                "cloudflare_configured": !cfg.cloudflare_r2_bucket.trim().is_empty()
                    && !cfg.cloudflare_api_token.trim().is_empty(),
                "has_access_token": !cfg.share_access_token.trim().is_empty(),
                "active_shares_count": cfg.shared_projects.len(),
            }))
        }
        "config-set" | "configure" => {
            let cfg = init_or_load_config().map_err(|e| e.to_string())?;
            let mut settings = ShareSettings::from(&cfg);
            if let Some(v) = opt_flag(cmd, "server-url") {
                settings.share_server_url = v.trim().to_string();
            }
            if let Some(v) = opt_flag(cmd, "r2-account-id") {
                settings.cloudflare_account_id = v.trim().to_string();
            }
            if let Some(v) = opt_flag(cmd, "r2-bucket") {
                settings.cloudflare_r2_bucket = v.trim().to_string();
            }
            if let Some(v) = opt_flag(cmd, "r2-api-token") {
                settings.cloudflare_api_token = v.trim().to_string();
            }
            if let Some(v) = opt_flag(cmd, "r2-pages-url") {
                settings.cloudflare_pages_url = v.trim().to_string();
            }
            if let Some(v) = opt_flag(cmd, "access-token") {
                settings.share_access_token = v.trim().to_string();
            }
            save_config_share_settings(&settings).map_err(|e| e.to_string())?;
            Ok(json!({
                "share_server_url": settings.share_server_url,
                "cloudflare_account_id": settings.cloudflare_account_id,
                "cloudflare_r2_bucket": settings.cloudflare_r2_bucket,
                "cloudflare_pages_url": settings.cloudflare_pages_url,
                "has_access_token": !settings.share_access_token.trim().is_empty(),
            }))
        }
        "login" => {
            let mut cfg = init_or_load_config().map_err(|e| e.to_string())?;
            let server_url = opt_flag(cmd, "server-url")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| cfg.share_server_url.clone());
            let email = req_flag(cmd, "email")?.trim().to_string();
            let password = req_flag(cmd, "password")?.to_string();

            let rx = dispatch_share_future(login_testrix_server(
                server_url.clone(),
                email,
                password,
            ));
            let res = rx
                .blocking_recv()
                .map_err(|_| "Login task cancelled".to_string())??;
            cfg.share_server_url = server_url.clone();
            cfg.share_email = res.email.clone();
            cfg.share_token = res.token;
            let settings = ShareSettings::from(&cfg);
            save_config_share_settings(&settings).map_err(|e| e.to_string())?;
            Ok(json!({
                "server_url": server_url,
                "email": res.email,
                "logged_in": true,
            }))
        }
        "publish" | "create" | "upload" | "sync" => {
            let project_ref = req_flag(cmd, "project")?;
            let ws = load_workspace(workspace_dir).map_err(|e| e.to_string())?;
            let project = find_project(&ws.projects, project_ref)?;
            let mut cfg = init_or_load_config().map_err(|e| e.to_string())?;
            let settings = ShareSettings::from(&cfg);

            let channel = match opt_flag(cmd, "channel") {
                Some(raw) => parse_share_channel(raw)?,
                None => resolve_default_share_channel(&settings),
            };
            let access_token = opt_flag(cmd, "access-token")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| settings.share_access_token.clone());

            let files = collect_project_yaml_files(&project.root_dir)?;
            let project_slug = project_slug_from_node(project);
            let project_name = project.doc.name.clone();
            let project_id = project.doc.id.clone();

            let target_info = match channel {
                ShareChannel::TestrixServer => settings.share_server_url.clone(),
                ShareChannel::CloudflareR2 => format!("r2://{}", settings.cloudflare_r2_bucket),
                ShareChannel::LocalNetwork => String::new(),
            };

            let rx = match channel {
                ShareChannel::TestrixServer => dispatch_share_future(upload_to_testrix_server(
                    settings.share_server_url,
                    settings.share_token,
                    project_slug.clone(),
                    project_name.clone(),
                    access_token,
                    files,
                )),
                ShareChannel::CloudflareR2 => dispatch_share_future(upload_to_cloudflare_r2(
                    settings.cloudflare_account_id,
                    settings.cloudflare_r2_bucket,
                    settings.cloudflare_api_token,
                    settings.cloudflare_pages_url,
                    project_slug.clone(),
                    access_token,
                    files,
                )),
                ShareChannel::LocalNetwork => unreachable!(),
            };

            let res = rx
                .blocking_recv()
                .map_err(|_| "Share upload task cancelled".to_string())??;

            let channel_label = res.channel.label().to_string();
            let share_id = if let Some(existing) = cfg.shared_projects.iter_mut().find(|r| {
                r.project_id == project_id && r.channel == channel_label
            }) {
                existing.project_name = project_name.clone();
                existing.project_slug = project_slug.clone();
                existing.share_url = res.share_url.clone();
                existing.target_info = target_info.clone();
                existing.id.clone()
            } else {
                let id = generate_id("share");
                cfg.shared_projects.push(PersistedShareRecord {
                    id: id.clone(),
                    project_id: project_id.clone(),
                    project_name: project_name.clone(),
                    project_slug: project_slug.clone(),
                    channel: channel_label.clone(),
                    share_url: res.share_url.clone(),
                    target_info,
                });
                id
            };

            save_config_shared_projects(&cfg.shared_projects).map_err(|e| e.to_string())?;
            Ok(json!({
                "id": share_id,
                "project_id": project_id,
                "project_name": project_name,
                "project_slug": project_slug,
                "channel": channel_label,
                "share_url": res.share_url,
                "files_uploaded": res.files_uploaded,
            }))
        }
        "revoke" | "delete" => {
            let mut cfg = init_or_load_config().map_err(|e| e.to_string())?;
            let settings = ShareSettings::from(&cfg);
            let share_id = opt_flag(cmd, "share").map(str::trim);
            let project_ref = opt_flag(cmd, "project").map(str::trim);
            let channel_filter = opt_flag(cmd, "channel")
                .map(parse_share_channel)
                .transpose()?;

            if share_id.is_none() && project_ref.is_none() {
                return Err("Specify `--share <id>` or `--project <id-or-name>` to revoke".to_string());
            }

            let mut to_revoke = Vec::new();
            let mut remaining = Vec::new();
            for rec in cfg.shared_projects.drain(..) {
                let matches_id = share_id.is_some_and(|id| rec.id == id);
                let matches_proj = project_ref.is_some_and(|p| {
                    rec.project_id == p || rec.project_name == p || rec.project_slug == p
                });
                let matches_channel = channel_filter
                    .is_none_or(|ch| ShareChannel::from_label(&rec.channel) == Some(ch));

                if (matches_id || matches_proj) && matches_channel {
                    to_revoke.push(rec);
                } else {
                    remaining.push(rec);
                }
            }

            if to_revoke.is_empty() {
                return Err("No matching share record found".to_string());
            }

            let ws = load_workspace(workspace_dir).ok();
            let mut revoked_out = Vec::new();
            for rec in &to_revoke {
                let channel = ShareChannel::from_label(&rec.channel).ok_or_else(|| {
                    format!("Unknown share channel `{}` on record `{}`", rec.channel, rec.id)
                })?;
                match channel {
                    ShareChannel::TestrixServer => {
                        let rx = dispatch_share_future(revoke_testrix_server_share(
                            settings.share_server_url.clone(),
                            settings.share_token.clone(),
                            rec.project_slug.clone(),
                        ));
                        rx.blocking_recv()
                            .map_err(|_| "Revoke task cancelled".to_string())??;
                    }
                    ShareChannel::CloudflareR2 => {
                        let file_paths = ws
                            .as_ref()
                            .and_then(|tree| {
                                tree.projects
                                    .iter()
                                    .find(|p| p.doc.id == rec.project_id)
                                    .and_then(|p| collect_project_yaml_files(&p.root_dir).ok())
                            })
                            .map(|files| files.into_iter().map(|f| f.path).collect())
                            .unwrap_or_default();
                        let rx = dispatch_share_future(revoke_cloudflare_r2_share(
                            settings.cloudflare_account_id.clone(),
                            settings.cloudflare_r2_bucket.clone(),
                            settings.cloudflare_api_token.clone(),
                            rec.project_slug.clone(),
                            file_paths,
                        ));
                        rx.blocking_recv()
                            .map_err(|_| "Revoke task cancelled".to_string())??;
                    }
                    ShareChannel::LocalNetwork => {}
                }
                revoked_out.push(json!({
                    "id": rec.id,
                    "project_id": rec.project_id,
                    "project_name": rec.project_name,
                    "channel": rec.channel,
                }));
            }

            save_config_shared_projects(&remaining).map_err(|e| e.to_string())?;
            Ok(json!({ "revoked": revoked_out }))
        }
        other => Err(format!(
            "Unknown share action `{other}` (expected list | status | config-set | login | publish | revoke)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn crud_for_project_item_profile_and_variables_with_legal_checks() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();

        // Reject illegal project base_url
        let bad_proj = execute_cli_api(
            ws,
            &s(&["project", "create", "--name", "Orders", "--base-url", "ftp://bad"]),
        );
        assert!(bad_proj.is_err());

        // Create project
        let created = execute_cli_api(
            ws,
            &s(&[
                "project",
                "create",
                "--name",
                "Orders",
                "--base-url",
                "https://orders.test",
            ]),
        )
        .unwrap();
        assert!(created.contains("Orders"));

        // Create item from cURL script
        let item = execute_cli_api(
            ws,
            &s(&[
                "item",
                "create",
                "--project",
                "Orders",
                "--name",
                "Create Order",
                "--script",
                r#"curl -X POST 'https://orders.test/v1/orders' -H 'Content-Type: application/json' --data-raw '{"qty":2}'"#,
            ]),
        )
        .unwrap();
        assert!(item.contains("POST"));

        // Reject invalid HTTP method on update
        let bad_method = execute_cli_api(
            ws,
            &s(&[
                "item",
                "update",
                "--project",
                "Orders",
                "--item",
                "Create Order",
                "--method",
                "BOGUS",
            ]),
        );
        assert!(bad_method.is_err());

        // Create profiles and merge variables
        execute_cli_api(
            ws,
            &s(&["profile", "create", "--project", "Orders", "--name", "dev"]),
        )
        .unwrap();
        execute_cli_api(
            ws,
            &s(&["profile", "create", "--project", "Orders", "--name", "prod"]),
        )
        .unwrap();

        // Reject illegal variable key with spaces
        let bad_var = execute_cli_api(
            ws,
            &s(&[
                "var",
                "create",
                "--project",
                "Orders",
                "--profile",
                "dev",
                "--key",
                "bad key",
                "--value",
                "123",
            ]),
        );
        assert!(bad_var.is_err());

        // Create & merge variable
        execute_cli_api(
            ws,
            &s(&[
                "var",
                "create",
                "--project",
                "Orders",
                "--profile",
                "dev",
                "--key",
                "api_token",
                "--value",
                "dev-tok",
                "--secret",
                "true",
            ]),
        )
        .unwrap();

        let merged = execute_cli_api(
            ws,
            &s(&[
                "profile",
                "merge",
                "--project",
                "Orders",
                "--profile",
                "prod",
                "--from",
                "dev",
            ]),
        )
        .unwrap();
        assert!(merged.contains(r#""added": 1"#));

        // Delete variable
        execute_cli_api(
            ws,
            &s(&[
                "var",
                "delete",
                "--project",
                "Orders",
                "--profile",
                "prod",
                "--key",
                "api_token",
            ]),
        )
        .unwrap();
    }

    #[test]
    fn share_channel_parsing_and_validation() {
        assert_eq!(parse_share_channel("server").unwrap(), ShareChannel::TestrixServer);
        assert_eq!(parse_share_channel("r2").unwrap(), ShareChannel::CloudflareR2);
        assert!(parse_share_channel("local").is_err());
        assert!(parse_share_channel("unknown").is_err());

        let dir = TempDir::new().unwrap();
        let bad = execute_cli_api(dir.path(), &s(&["share", "revoke"]));
        assert!(bad.is_err());
    }

    #[test]
    fn package_crud_and_folder_item_management() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();

        execute_cli_api(ws, &s(&["project", "create", "--name", "Orders"])).unwrap();
        execute_cli_api(
            ws,
            &s(&[
                "item",
                "create",
                "--project",
                "Orders",
                "--name",
                "List Orders",
                "--method",
                "GET",
                "--url",
                "{{base_url}}/orders",
            ]),
        )
        .unwrap();

        let created = execute_cli_api(ws, &s(&["package", "create", "--name", "Smoke"])).unwrap();
        assert!(created.contains(r#""name": "Smoke""#));

        execute_cli_api(
            ws,
            &s(&[
                "package",
                "update",
                "--package",
                "Smoke",
                "--name",
                "Regression",
            ]),
        )
        .unwrap();

        execute_cli_api(
            ws,
            &s(&[
                "package",
                "add-folder",
                "--package",
                "Regression",
                "--name",
                "Auth",
            ]),
        )
        .unwrap();

        execute_cli_api(
            ws,
            &s(&[
                "package",
                "update-folder",
                "--package",
                "Regression",
                "--folder",
                "Auth",
                "--name",
                "Core",
            ]),
        )
        .unwrap();

        execute_cli_api(
            ws,
            &s(&[
                "package",
                "add-item",
                "--package",
                "Regression",
                "--folder",
                "Core",
                "--project",
                "Orders",
                "--item",
                "List Orders",
            ]),
        )
        .unwrap();

        let list = execute_cli_api(ws, &s(&["package", "list"])).unwrap();
        assert!(list.contains(r#""name": "Regression""#));
        assert!(list.contains(r#""totalRequestCount": 1"#));

        let fetched =
            execute_cli_api(ws, &s(&["package", "get", "--package", "Regression"])).unwrap();
        assert!(fetched.contains("List Orders"));

        execute_cli_api(
            ws,
            &s(&[
                "package",
                "remove-item",
                "--package",
                "Regression",
                "--folder",
                "Core",
                "--item",
                "List Orders",
            ]),
        )
        .unwrap();

        execute_cli_api(
            ws,
            &s(&[
                "package",
                "delete-folder",
                "--package",
                "Regression",
                "--folder",
                "Core",
            ]),
        )
        .unwrap();

        execute_cli_api(ws, &s(&["package", "delete", "--package", "Regression"])).unwrap();

        let empty_list = execute_cli_api(ws, &s(&["package", "list"])).unwrap();
        assert!(empty_list.contains("[]"));
    }
}
