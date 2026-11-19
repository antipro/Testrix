use std::{
    collections::HashSet,
    net::UdpSocket,
    sync::{Arc, RwLock},
};

use serde::Serialize;
use testrix_core::{FolderNode, ProjectNode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

/// An outbound IPv4 address discovered on a local network adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAdapterIp {
    pub interface_name: String,
    pub ip: String,
}

impl NetworkAdapterIp {
    pub fn display_label(&self) -> String {
        if self.interface_name.is_empty() {
            self.ip.clone()
        } else {
            format!("{} ({})", self.ip, self.interface_name)
        }
    }
}

/// Handle to a running local-network API documentation HTTP server.
#[derive(Debug, Clone)]
pub struct LocalShareHandle {
    pub share_url: String,
    pub bind_addr: String,
    project_slug: String,
    project_json: Arc<RwLock<String>>,
    cancel: CancellationToken,
}

impl LocalShareHandle {
    /// Stops the local HTTP server and closes its listening socket.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Updates the served project snapshot after local edits are saved.
    pub fn update_project(&self, project: &ProjectNode) {
        if let Ok(next_json) = serialize_local_project(project, &self.project_slug)
            && let Ok(mut guard) = self.project_json.write()
        {
            *guard = next_json;
        }
    }
}

/// Enumerates local IPv4 addresses across network adapters (plus `127.0.0.1` fallback).
pub fn list_outbound_ipv4_addresses() -> Vec<NetworkAdapterIp> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    // 1. Try default outbound IP via UDP socket connect (does not send any packets).
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0")
        && sock.connect("8.8.8.8:80").is_ok()
        && let Ok(local_addr) = sock.local_addr()
    {
        let ip = local_addr.ip().to_string();
        if !ip.starts_with("127.") && seen.insert(ip.clone()) {
            results.push(NetworkAdapterIp {
                interface_name: "default".to_string(),
                ip,
            });
        }
    }

    // 2. Windows: parse `ipconfig` (works across locales since "IPv4" is ASCII) and fallback to PowerShell.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        if let Ok(out) = std::process::Command::new("ipconfig")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            && out.status.success()
        {
            let text = decode_windows_console_output(&out.stdout);
            let mut current_iface = String::new();
            for line in text.lines() {
                let raw = line.trim_end();
                if raw.is_empty() {
                    continue;
                }
                if !raw.starts_with(char::is_whitespace) && raw.ends_with(':') {
                    current_iface = raw.trim_end_matches(':').trim().to_string();
                    continue;
                }
                let trimmed = raw.trim();
                if let Some((key, val)) = trimmed.split_once(':') {
                    if key.contains("IPv4") || key.contains("IP Address") {
                        let ip = val
                            .split('(')
                            .next()
                            .unwrap_or("")
                            .trim();
                        if is_valid_ipv4(ip) && !ip.starts_with("127.") && seen.insert(ip.to_string()) {
                            results.push(NetworkAdapterIp {
                                interface_name: current_iface.clone(),
                                ip: ip.to_string(),
                            });
                        }
                    }
                }
            }
        }

        if results.len() <= 1
            && let Ok(out) = std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-NetIPAddress -AddressFamily IPv4 | ForEach-Object { \"$($_.InterfaceAlias)|$($_.IPAddress)\" }",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
            && out.status.success()
        {
            let text = decode_windows_console_output(&out.stdout);
            for line in text.lines() {
                if let Some((iface, ip)) = line.trim().split_once('|') {
                    let ip = ip.trim();
                    if is_valid_ipv4(ip) && !ip.starts_with("127.") && seen.insert(ip.to_string()) {
                        results.push(NetworkAdapterIp {
                            interface_name: iface.trim().to_string(),
                            ip: ip.to_string(),
                        });
                    }
                }
            }
        }
    }

    // 3. Parse `ifconfig` on macOS/BSD/Unix for all adapter IPv4 addresses.
    #[cfg(not(target_os = "windows"))]
    if let Ok(out) = std::process::Command::new("ifconfig").output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut current_iface = String::new();
        for line in text.lines() {
            if !line.starts_with(char::is_whitespace)
                && let Some(colon_idx) = line.find(':')
            {
                current_iface = line[..colon_idx].trim().to_string();
                continue;
            }
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("inet ") {
                let ip = rest.split_whitespace().next().unwrap_or("").trim();
                if is_valid_ipv4(ip) && !ip.starts_with("127.") && seen.insert(ip.to_string()) {
                    results.push(NetworkAdapterIp {
                        interface_name: current_iface.clone(),
                        ip: ip.to_string(),
                    });
                }
            }
        }
    }

    // 4. Parse `ip -4 -o addr show` on Linux if available.
    #[cfg(not(target_os = "windows"))]
    if results.len() <= 1
        && let Ok(out) = std::process::Command::new("ip")
            .args(["-4", "-o", "addr", "show"])
            .output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let iface = parts[1];
                let ip = parts[3].split('/').next().unwrap_or("");
                if is_valid_ipv4(ip) && !ip.starts_with("127.") && seen.insert(ip.to_string()) {
                    results.push(NetworkAdapterIp {
                        interface_name: iface.to_string(),
                        ip: ip.to_string(),
                    });
                }
            }
        }
    }

    // Always include localhost as a fallback option.
    if seen.insert("127.0.0.1".to_string()) {
        results.push(NetworkAdapterIp {
            interface_name: "loopback".to_string(),
            ip: "127.0.0.1".to_string(),
        });
    }

    results
}

fn is_valid_ipv4(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>().is_ok()
}

/// Decodes Windows console output that may be UTF-8 or the system's OEM code page (e.g., GBK/CP936).
#[cfg(target_os = "windows")]
fn decode_windows_console_output(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    const CP_OEMCP: u32 = 1;
    unsafe extern "system" {
        fn MultiByteToWideChar(
            code_page: u32,
            dw_flags: u32,
            lp_multi_byte_str: *const u8,
            cb_multi_byte: i32,
            lp_wide_char_str: *mut u16,
            cch_wide_char: i32,
        ) -> i32;
    }
    let Ok(len) = i32::try_from(bytes.len()) else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    // SAFETY: `bytes.as_ptr()` is valid for `len` bytes and `lp_wide_char_str` is null when querying length.
    let wide_len = unsafe {
        MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0)
    };
    if wide_len <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; wide_len as usize];
    // SAFETY: `wide` has capacity `wide_len` `u16` elements.
    let written = unsafe {
        MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), wide_len)
    };
    if written <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    String::from_utf16_lossy(&wide[..written as usize])
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalProjectPayload<'a> {
    slug: &'a str,
    doc: &'a testrix_core::ProjectDoc,
    environments: Vec<LocalEnvPayload<'a>>,
    folders: Vec<LocalFolderPayload<'a>>,
    requests: Vec<LocalRequestPayload<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalEnvPayload<'a> {
    name: &'a str,
    variables: &'a [testrix_core::EnvVariable],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalFolderPayload<'a> {
    rel_path: String,
    doc: &'a testrix_core::FolderDoc,
    folders: Vec<LocalFolderPayload<'a>>,
    requests: Vec<LocalRequestPayload<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalRequestPayload<'a> {
    rel_path: String,
    doc: &'a testrix_core::HttpRequestDoc,
}

fn map_folder<'a>(folder: &'a FolderNode, prefix: &str) -> LocalFolderPayload<'a> {
    let rel = if prefix.is_empty() {
        folder.doc.id.clone()
    } else {
        format!("{prefix}/{}", folder.doc.id)
    };
    LocalFolderPayload {
        rel_path: rel.clone(),
        doc: &folder.doc,
        folders: folder.folders.iter().map(|f| map_folder(f, &rel)).collect(),
        requests: folder
            .requests
            .iter()
            .map(|r| LocalRequestPayload {
                rel_path: format!("{rel}/{}", r.doc.id),
                doc: &r.doc,
            })
            .collect(),
    }
}

fn serialize_local_project(project: &ProjectNode, project_slug: &str) -> Result<String, String> {
    let payload = LocalProjectPayload {
        slug: project_slug,
        doc: &project.doc,
        environments: project
            .environments
            .iter()
            .map(|e| LocalEnvPayload {
                name: &e.doc.name,
                variables: &e.doc.variables,
            })
            .collect(),
        folders: project
            .folders
            .iter()
            .map(|f| map_folder(f, ""))
            .collect(),
        requests: project
            .requests
            .iter()
            .map(|r| LocalRequestPayload {
                rel_path: r.doc.id.clone(),
                doc: &r.doc,
            })
            .collect(),
    };
    serde_json::to_string(&payload).map_err(|err| format!("Failed serializing project JSON: {err}"))
}

/// Starts a lightweight embedded HTTP server on `bind_ip:port` serving `project`'s API docs.
pub fn start_local_share_server(
    bind_ip: String,
    port: u16,
    project: ProjectNode,
    project_slug: String,
) -> Result<LocalShareHandle, String> {
    let project_json = serialize_local_project(&project, &project_slug)?;
    let project_json_lock = Arc::new(RwLock::new(project_json));
    let json_for_server = project_json_lock.clone();

    let cancel = CancellationToken::new();
    let cancel_bg = cancel.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<std::net::SocketAddr, String>>();

    let bind_target = format!("{bind_ip}:{port}");
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                let _ = tx.send(Err(format!("Failed starting Tokio runtime: {err}")));
                return;
            }
        };

        rt.block_on(async move {
            let listener = match TcpListener::bind(&bind_target).await {
                Ok(l) => l,
                Err(err) => {
                    let _ = tx.send(Err(format!("Failed binding {bind_target}: {err}")));
                    return;
                }
            };
            let local_addr = match listener.local_addr() {
                Ok(a) => a,
                Err(err) => {
                    let _ = tx.send(Err(format!("Failed reading local_addr: {err}")));
                    return;
                }
            };
            let _ = tx.send(Ok(local_addr));

            loop {
                tokio::select! {
                    _ = cancel_bg.cancelled() => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else { break; };
                        let json_data = json_for_server.clone();
                        tokio::spawn(async move {
                            let mut buf = [0u8; 4096];
                            let n = socket.read(&mut buf).await.unwrap_or(0);
                            if n == 0 { return; }
                            let req_str = String::from_utf8_lossy(&buf[..n]);
                            let first_line = req_str.lines().next().unwrap_or("");
                            let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                            let json_snapshot;
                            let (status, content_type, body) = if path.starts_with("/api/project") {
                                json_snapshot = json_data.read().map(|g| g.clone()).unwrap_or_default();
                                ("200 OK", "application/json; charset=utf-8", json_snapshot.as_str())
                            } else {
                                ("200 OK", "text/html; charset=utf-8", LOCAL_VIEWER_HTML)
                            };

                            let resp = format!(
                                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = socket.write_all(resp.as_bytes()).await;
                        });
                    }
                }
            }
        });
    });

    let bound_addr = rx
        .recv()
        .map_err(|_| "Local share server thread exited unexpectedly".to_string())??;
    let share_url = format!("http://{}:{}", bound_addr.ip(), bound_addr.port());
    Ok(LocalShareHandle {
        share_url,
        bind_addr: bound_addr.to_string(),
        project_slug,
        project_json: project_json_lock,
        cancel,
    })
}

const LOCAL_VIEWER_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>Testrix Local API Share</title>
<style>
:root{--bg:#0f1117;--surface:#161922;--surface2:#1e222e;--border:#2a2f3d;--text:#e6e9ef;--muted:#939ab0;--accent:#3b82f6}
*{box-sizing:border-box}
html,body{margin:0;padding:0;height:100%;background:var(--bg);color:var(--text);font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;font-size:14px;line-height:1.5}
.topbar{height:52px;background:var(--surface);border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between;padding:0 16px;gap:12px}
.brand{font-weight:700;display:flex;align-items:center;gap:8px}
.dot{width:9px;height:9px;border-radius:50%;background:#22c55e}
.select,.input{background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:6px;padding:5px 10px;font-size:13px}
.layout{height:calc(100vh - 52px);display:flex;overflow:hidden}
.sidebar{width:290px;background:var(--surface);border-right:1px solid var(--border);overflow-y:auto;padding:12px 8px}
.content{flex:1;overflow-y:auto;padding:28px 34px}
.tree-item{display:flex;align-items:center;gap:8px;padding:6px 10px;border-radius:6px;cursor:pointer}
.tree-item:hover{background:var(--surface2)}
.tree-item.active{background:rgba(59,130,246,0.16);color:#93c5fd}
.children{padding-left:14px}
.badge{font-size:10px;font-weight:700;padding:2px 6px;border-radius:4px;text-transform:uppercase}
.GET{background:rgba(34,197,94,0.16);color:#22c55e}
.POST{background:rgba(59,130,246,0.18);color:#60a5fa}
.PUT{background:rgba(245,158,11,0.18);color:#f59e0b}
.PATCH{background:rgba(168,85,247,0.18);color:#c084fc}
.DELETE{background:rgba(239,68,68,0.18);color:#f87171}
.url-bar{display:flex;align-items:center;gap:10px;background:var(--surface);border:1px solid var(--border);border-radius:8px;padding:10px 14px;margin:16px 0 22px;font-family:ui-monospace,Menlo,monospace;font-size:13px;word-break:break-all}
.section{margin-bottom:22px}
.section-title{font-size:12px;font-weight:600;text-transform:uppercase;color:var(--muted);margin-bottom:8px}
table{width:100%;border-collapse:collapse;background:var(--surface);border:1px solid var(--border);border-radius:8px;overflow:hidden}
th,td{text-align:left;padding:8px 12px;border-bottom:1px solid var(--border);font-size:13px}
th{background:var(--surface2);color:var(--muted)}
pre{background:var(--surface);border:1px solid var(--border);border-radius:8px;padding:12px;overflow-x:auto;font-family:ui-monospace,Menlo,monospace;font-size:13px;white-space:pre-wrap;word-break:break-word}
.btn{background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:6px;padding:5px 10px;font-size:12px;cursor:pointer}
.btn:hover{border-color:var(--accent)}
.btn.active{background:var(--accent);border-color:var(--accent);color:#fff}
.doc-header{display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap}
.code-hdr{display:flex;align-items:center;justify-content:space-between;gap:8px;flex-wrap:wrap;margin-bottom:8px}
.code-tabs{display:flex;gap:6px;flex-wrap:wrap}
.pill{display:inline-block;padding:2px 8px;border-radius:999px;font-size:11px;background:var(--surface2);border:1px solid var(--border);color:var(--muted)}
.meta-cell{display:flex;align-items:center;gap:6px}
.meta-branch{color:var(--muted);font-family:ui-monospace,Menlo,monospace}
.ex-card{background:var(--surface);border:1px solid var(--border);border-radius:8px;overflow:hidden;margin-bottom:12px}
.ex-card pre{border:none;border-top:1px solid var(--border);border-radius:0;margin:0}
.ex-hdr{display:flex;align-items:center;gap:10px;padding:10px 14px;background:var(--surface2)}
.st-2xx{background:rgba(74,222,128,.16);color:#4ade80}
.st-3xx{background:rgba(96,165,250,.16);color:#60a5fa}
.st-err{background:rgba(248,113,113,.16);color:#f87171}
</style>
</head>
<body>
<header class="topbar">
  <div class="brand"><span class="dot"></span><span id="projTitle">Testrix Local Share</span></div>
  <div style="display:flex;gap:10px;align-items:center;">
    <input id="searchInput" class="input" type="search" placeholder="Filter endpoints..." />
    <select id="envSelect" class="select" style="display:none"></select>
  </div>
</header>
<main class="layout">
  <aside class="sidebar" id="sidebar"></aside>
  <section class="content" id="content"></section>
</main>
<script>
let project = null, sel = null, envIdx = 0, query = '', codeTarget = 'curl';
const esc = s => String(s ?? '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
function envMap(){
  const m = new Map();
  for(const v of project?.doc?.variables || []) if(v.enabled!==false && v.key) m.set(v.key, v.value||'');
  const env = (project?.environments||[])[envIdx];
  for(const v of env?.variables || []) if(v.enabled!==false && v.key) m.set(v.key, v.secret?'••••••':(v.value||''));
  return m;
}
function resolve(s){ const m = envMap(); return String(s||'').replace(/\{\{([^}]+)\}\}/g,(a,k)=>m.has(k.trim())?m.get(k.trim()):a); }
function kvTable(rows){
  if(!rows||!rows.length) return '<div style="color:var(--muted)">None</div>';
  return '<table><thead><tr><th>Key</th><th>Value</th><th>Description</th></tr></thead><tbody>'+rows.map(r=>`<tr><td><code>${esc(r.name??r.key??'')}</code></td><td><code>${esc(resolve(r.value||''))}</code></td><td style="color:var(--muted)">${esc(r.description||'')}</td></tr>`).join('')+'</tbody></table>';
}
function buildReqUrl(r){
  let u = r.url||'';
  if(u && !/^https?:\/\//i.test(u) && !u.startsWith('{{') && project?.doc?.baseUrl) u = project.doc.baseUrl.replace(/\/+$/,'')+'/'+u.replace(/^\/+/,'');
  return resolve(u);
}
const escPosix = s => String(s).replace(/'/g,"'\\''");
const escCmd = s => String(s).replace(/"/g,'\\"').replace(/\r/g,'').replace(/\n/g,'\\n');
const escPs1 = s => String(s).replace(/'/g,"''");
const escPs2 = s => String(s).replace(/`/g,'``').replace(/"/g,'`"').replace(/\$/g,'`$');
function genCode(doc, target){
  const r = doc?.request||{};
  const method = (r.method||'GET').toUpperCase();
  let url = buildReqUrl(r);
  const params = (r.params||[]).filter(p=>p.enabled!==false && (p.name||p.key));
  if(params.length){
    const qs = params.map(p=>`${encodeURIComponent(resolve(p.name||p.key))}=${encodeURIComponent(resolve(p.value||''))}`).join('&');
    url += (url.includes('?')?'&':'?')+qs;
  }
  const headers = (r.headers||[]).filter(h=>h.enabled!==false && (h.name||h.key)).map(h=>({name:resolve(h.name||h.key),value:resolve(h.value||'')}));
  const body = r.body||{type:'none'};
  if(!headers.some(h=>h.name.toLowerCase()==='content-type')){
    if(body.type==='json') headers.push({name:'Content-Type',value:'application/json'});
    else if(body.type==='raw') headers.push({name:'Content-Type',value:body.rawFormat==='xml'?'application/xml':body.rawFormat==='html'?'text/html':'text/plain'});
    else if(body.type==='formUrlEncoded') headers.push({name:'Content-Type',value:'application/x-www-form-urlencoded'});
    else if(body.type==='binary' && body.contentType) headers.push({name:'Content-Type',value:resolve(body.contentType)});
  }
  if(target==='curl-win'){
    const ls=[`curl.exe -X ${method} "${escCmd(url)}"`];
    for(const h of headers) ls.push(`  -H "${escCmd(h.name)}: ${escCmd(h.value)}"`);
    if(body.type==='json'||body.type==='raw') ls.push(`  --data-raw "${escCmd(resolve(body.content||''))}"`);
    else if(body.type==='formUrlEncoded') for(const f of body.fields||[]) if(f.enabled!==false&&(f.name||f.key)) ls.push(`  --data-urlencode "${escCmd(resolve(f.name||f.key))}=${escCmd(resolve(f.value||''))}"`);
    else if(body.type==='multipart') for(const f of body.fields||[]) if(f.enabled!==false&&(f.name||f.key)) ls.push(f.fieldType==='file'?`  -F "${escCmd(resolve(f.name||f.key))}=@${escCmd(resolve(f.value||''))}"`:`  -F "${escCmd(resolve(f.name||f.key))}=${escCmd(resolve(f.value||''))}"`);
    else if(body.type==='binary'&&body.filePath) ls.push(`  --data-binary "@${escCmd(resolve(body.filePath))}"`);
    return ls.join(' ^\n');
  }
  if(target==='wget'){
    const ls=[`wget --method=${method}`];
    for(const h of headers) ls.push(`  --header='${escPosix(h.name)}: ${escPosix(h.value)}'`);
    if(body.type==='json'||body.type==='raw') ls.push(`  --body-data='${escPosix(resolve(body.content||''))}'`);
    else if(body.type==='formUrlEncoded'){ const enc=(body.fields||[]).filter(f=>f.enabled!==false&&(f.name||f.key)).map(f=>`${encodeURIComponent(resolve(f.name||f.key))}=${encodeURIComponent(resolve(f.value||''))}`).join('&'); ls.push(`  --body-data='${escPosix(enc)}'`); }
    else if(body.type==='multipart') ls.push('  # Note: wget does not natively build multipart/form-data; use cURL');
    else if(body.type==='binary'&&body.filePath) ls.push(`  --body-file='${escPosix(resolve(body.filePath))}'`);
    ls.push(`  -qO- '${escPosix(url)}'`);
    return ls.join(' \\\n');
  }
  if(target==='powershell'){
    const out=[], hl=[]; let ct=null;
    for(const h of headers){ if(h.name.toLowerCase()==='content-type') ct=h.value; else hl.push(`    "${escPs2(h.name)}" = "${escPs2(h.value)}"`); }
    if(hl.length) out.push('$headers = @{',...hl,'}');
    let hasBody=false;
    if(body.type==='json'||body.type==='raw'){ hasBody=true; const c=resolve(body.content||''); out.push(c.includes('\n')?`$body = @'\n${c}\n'@`:`$body = '${escPs1(c)}'`); }
    else if(body.type==='formUrlEncoded'){ hasBody=true; out.push('$body = @{'); for(const f of body.fields||[]) if(f.enabled!==false&&(f.name||f.key)) out.push(`    "${escPs2(resolve(f.name||f.key))}" = "${escPs2(resolve(f.value||''))}"`); out.push('}'); }
    else if(body.type==='binary'&&body.filePath){ hasBody=true; out.push(`$body = [System.IO.File]::ReadAllBytes('${escPs1(resolve(body.filePath))}')`); }
    let inv=`$response = Invoke-WebRequest -Uri '${escPs1(url)}' -Method ${method} -UseBasicParsing`;
    if(hl.length) inv+=' -Headers $headers'; if(ct) inv+=` -ContentType '${escPs1(ct)}'`; if(hasBody) inv+=' -Body $body';
    out.push(inv); return out.join('\n');
  }
  const ls=[`curl -X ${method} '${escPosix(url)}'`];
  for(const h of headers) ls.push(`  -H '${escPosix(h.name)}: ${escPosix(h.value)}'`);
  if(body.type==='json'||body.type==='raw') ls.push(`  --data-raw '${escPosix(resolve(body.content||''))}'`);
  else if(body.type==='formUrlEncoded') for(const f of body.fields||[]) if(f.enabled!==false&&(f.name||f.key)) ls.push(`  --data-urlencode '${escPosix(resolve(f.name||f.key))}=${escPosix(resolve(f.value||''))}'`);
  else if(body.type==='multipart') for(const f of body.fields||[]) if(f.enabled!==false&&(f.name||f.key)) ls.push(f.fieldType==='file'?`  -F '${escPosix(resolve(f.name||f.key))}=@${escPosix(resolve(f.value||''))}'`:`  -F '${escPosix(resolve(f.name||f.key))}=${escPosix(resolve(f.value||''))}'`);
  else if(body.type==='binary'&&body.filePath) ls.push(`  --data-binary '@${escPosix(resolve(body.filePath))}'`);
  return ls.join(' \\\n');
}
function renderMeta(meta,title){
  if(!Array.isArray(meta)||!meta.length) return '';
  const rows=[];
  function walk(fs,d){
    for(const f of fs||[]){
      rows.push(`<tr><td><div class="meta-cell" style="padding-left:${d*18}px">${d>0?'<span class="meta-branch">└─</span>':''}<code>${esc(f.name)}</code></div></td><td><span class="pill">${esc(f.type||'any')}</span></td><td><code style="color:var(--muted)">${esc(f.example||'—')}</code></td><td style="color:var(--muted)">${esc(f.description||'')}</td></tr>`);
      if(Array.isArray(f.children)&&f.children.length) walk(f.children,d+1);
    }
  }
  walk(meta,0);
  return `<div class="section"><div class="section-title">${esc(title||'Response Meta Info')}</div><table><thead><tr><th>Field</th><th>Type</th><th>Example</th><th>Description</th></tr></thead><tbody>${rows.join('')}</tbody></table></div>`;
}
function fmtEx(b,ct){ const s=String(b??''); if(String(ct||'').toLowerCase().includes('json')||s.trim().startsWith('{')||s.trim().startsWith('[')){ try{ return JSON.stringify(JSON.parse(s),null,2); }catch{} } return s; }
function renderExamples(exs){
  if(!Array.isArray(exs)||!exs.length) return '';
  return `<div class="section"><div class="section-title">Response Examples (${exs.length})</div>${exs.map(ex=>{
    const n=Number(ex.statusCode||0); const cls=n>=200&&n<300?'st-2xx':n>=300&&n<400?'st-3xx':n>=400?'st-err':'';
    return `<div class="ex-card"><div class="ex-hdr"><span class="badge ${cls}">${esc(ex.reason?`${n} ${ex.reason}`:String(n))}</span><strong>${esc(ex.name||'Example')}</strong>${ex.contentType?`<span class="pill">${esc(ex.contentType)}</span>`:''}</div><pre>${esc(fmtEx(ex.body,ex.contentType))}</pre></div>`;
  }).join('')}</div>`;
}
function render(){
  if(!project) return;
  const sb = document.getElementById('sidebar');
  let html = `<div class="tree-item ${sel?.type==='project'?'active':''}" onclick="selectProject()"><strong>${esc(project.doc.name)}</strong></div>`;
  function walkF(f){
    let inner = (f.folders||[]).map(walkF).join('') + (f.requests||[]).filter(r=>!query||(r.doc.name+r.doc.request.url).toLowerCase().includes(query)).map(r=>`<div class="tree-item ${sel?.id===r.relPath?'active':''}" onclick="selectReq('${esc(r.relPath)}')"><span class="badge ${esc((r.doc.request.method||'GET').toUpperCase())}">${esc((r.doc.request.method||'GET').toUpperCase())}</span><span>${esc(r.doc.name)}</span></div>`).join('');
    return `<div><div class="tree-item ${sel?.id===f.relPath?'active':''}" onclick="selectFolder('${esc(f.relPath)}')">▾ ${esc(f.doc.name)}</div><div class="children">${inner}</div></div>`;
  }
  html += (project.folders||[]).map(walkF).join('');
  html += (project.requests||[]).filter(r=>!query||(r.doc.name+r.doc.request.url).toLowerCase().includes(query)).map(r=>`<div class="tree-item ${sel?.id===r.relPath?'active':''}" onclick="selectReq('${esc(r.relPath)}')"><span class="badge ${esc((r.doc.request.method||'GET').toUpperCase())}">${esc((r.doc.request.method||'GET').toUpperCase())}</span><span>${esc(r.doc.name)}</span></div>`).join('');
  sb.innerHTML = html;

  const ct = document.getElementById('content');
  if(!sel || sel.type==='project'){
    ct.innerHTML = `<h1>${esc(project.doc.name)}</h1><p style="color:var(--muted)">${esc(project.doc.description||'')}</p><div class="section"><div class="section-title">Base URL</div><div class="url-bar">${esc(resolve(project.doc.baseUrl||''))}</div></div><div class="section"><div class="section-title">Default Headers</div>${kvTable(project.doc.defaultHeaders)}</div>`;
  } else if(sel.type==='folder'){
    const f = sel.data;
    ct.innerHTML = `<h1>${esc(f.doc.name)}</h1><p style="color:var(--muted)">${esc(f.doc.description||'')}</p><div class="section"><div class="section-title">Endpoints (${(f.requests||[]).length})</div>${(f.requests||[]).length?`<table><thead><tr><th>Method</th><th>Name</th><th>URL</th></tr></thead><tbody>${f.requests.map(r=>`<tr style="cursor:pointer" onclick="selectReq('${esc(r.relPath)}')"><td><span class="badge ${esc((r.doc.request.method||'GET').toUpperCase())}">${esc((r.doc.request.method||'GET').toUpperCase())}</span></td><td>${esc(r.doc.name)}</td><td><code>${esc(r.doc.request.url)}</code></td></tr>`).join('')}</tbody></table>`:'<div style="color:var(--muted)">Empty folder</div>'}</div>`;
  } else {
    const d = sel.data.doc, r = d.request||{};
    const m = (r.method||'GET').toUpperCase();
    const u = buildReqUrl(r);
    const targets=[['curl','cURL'],['curl-win','cURL (Win)'],['wget','wget'],['powershell','PowerShell']];
    ct.innerHTML = `<div class="doc-header"><h1>${esc(d.name)}</h1><div style="display:flex;gap:8px"><button class="btn" onclick="copyLink(this)">Copy Link</button><button class="btn" onclick="copyCode('curl',this)">Copy cURL</button></div></div><p style="color:var(--muted)">${esc(d.description||'')}</p><div class="url-bar"><span class="badge ${esc(m)}">${esc(m)}</span><span>${esc(u)}</span></div><div class="section"><div class="section-title">Query Params</div>${kvTable(r.params)}</div><div class="section"><div class="section-title">Headers</div>${kvTable(r.headers)}</div>${r.body&&r.body.type!=='none'?`<div class="section"><div class="section-title">Body</div><pre>${esc(r.body.content||JSON.stringify(r.body,null,2))}</pre></div>`:''}${renderMeta(r.bodyMeta,'Body Meta Info')}<div class="section"><div class="code-hdr"><div class="section-title" style="margin:0">Code Snippet</div><div class="code-tabs">${targets.map(([id,lbl])=>`<button class="btn ${codeTarget===id?'active':''}" onclick="setTarget('${id}')">${lbl}</button>`).join('')}<button class="btn" onclick="copyCode(codeTarget,this)">Copy</button></div></div><pre id="codeBlock">${esc(genCode(d,codeTarget))}</pre></div>${renderMeta(r.responseMeta,'Response Meta Info')}${renderExamples(r.responseExamples)}`;
  }
}
function findF(list,id){ for(const f of list||[]){ if(f.relPath===id) return f; const sub=findF(f.folders,id); if(sub) return sub; } }
function findR(target){
  const t = String(target||'').replace(/^\/+/,'');
  for(const r of project.requests||[]) if(r.relPath===t||r.doc?.id===t) return r;
  function w(fs){ for(const f of fs||[]){ for(const r of f.requests||[]) if(r.relPath===t||r.doc?.id===t) return r; const s=w(f.folders); if(s) return s; } }
  return w(project.folders);
}
window.selectProject = () => { sel={type:'project'}; history.pushState({},'','/'); render(); };
window.selectFolder = id => { const f=findF(project.folders,id); if(f){ sel={type:'folder',id,data:f}; render(); } };
window.selectReq = id => { const r=findR(id); if(r){ sel={type:'request',id:r.relPath,data:r}; history.pushState({},'','/r/'+r.relPath.split('/').map(encodeURIComponent).join('/')); render(); } };
window.setTarget = t => { codeTarget=t; render(); };
window.copyCode = (t,btn) => { if(sel?.type==='request'){ navigator.clipboard?.writeText(genCode(sel.data.doc,t)); const prev=btn.textContent; btn.textContent='Copied!'; setTimeout(()=>btn.textContent=prev,1400); } };
window.copyLink = btn => { if(sel?.type==='request'){ navigator.clipboard?.writeText(location.origin+'/r/'+sel.data.relPath.split('/').map(encodeURIComponent).join('/')); btn.textContent='Copied!'; setTimeout(()=>btn.textContent='Copy Link',1400); } };
function syncFromUrl(){
  if(!project) return;
  const sp = new URLSearchParams(location.search);
  const m = location.pathname.match(/^\/r\/(.+)$/);
  const target = m ? m[1].split('/').map(decodeURIComponent).join('/') : sp.get('request');
  const r = target ? findR(target) : null;
  sel = r ? {type:'request',id:r.relPath,data:r} : {type:'project'};
  render();
}
window.addEventListener('popstate', syncFromUrl);
document.getElementById('searchInput').oninput = e => { query = e.target.value.trim().toLowerCase(); render(); };
fetch('/api/project').then(r=>r.json()).then(d=>{
  project = d;
  document.getElementById('projTitle').textContent = d.doc.name || 'Testrix Local Share';
  const es = document.getElementById('envSelect');
  if((d.environments||[]).length){ es.style.display='inline-block'; es.innerHTML=d.environments.map((e,i)=>`<option value="${i}">${esc(e.name)}</option>`).join(''); es.onchange=()=>{ envIdx=Number(es.value); render(); }; }
  syncFromUrl();
});
</script>
</body>
</html>"#;
