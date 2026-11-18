use std::{collections::HashSet, net::UdpSocket};

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
    cancel: CancellationToken,
}

impl LocalShareHandle {
    /// Stops the local HTTP server and closes its listening socket.
    pub fn stop(&self) {
        self.cancel.cancel();
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
            let text = String::from_utf8_lossy(&out.stdout);
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
                    "Get-NetIPAddress -AddressFamily IPv4 | ForEach-Object { \"$($_.InterfaceAlias)|$($_.IPAddress)\" }",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout);
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

/// Starts a lightweight embedded HTTP server on `bind_ip:port` serving `project`'s API docs.
pub fn start_local_share_server(
    bind_ip: String,
    port: u16,
    project: ProjectNode,
    project_slug: String,
) -> Result<LocalShareHandle, String> {
    let payload = LocalProjectPayload {
        slug: &project_slug,
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
    let project_json = serde_json::to_string(&payload)
        .map_err(|err| format!("Failed serializing project JSON: {err}"))?;

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

            let json_arc = std::sync::Arc::new(project_json);
            loop {
                tokio::select! {
                    _ = cancel_bg.cancelled() => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else { break; };
                        let json_data = json_arc.clone();
                        tokio::spawn(async move {
                            let mut buf = [0u8; 4096];
                            let n = socket.read(&mut buf).await.unwrap_or(0);
                            if n == 0 { return; }
                            let req_str = String::from_utf8_lossy(&buf[..n]);
                            let first_line = req_str.lines().next().unwrap_or("");
                            let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                            let (status, content_type, body) = if path.starts_with("/api/project") {
                                ("200 OK", "application/json; charset=utf-8", json_data.as_str())
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
pre{background:var(--surface);border:1px solid var(--border);border-radius:8px;padding:12px;overflow-x:auto;font-family:ui-monospace,Menlo,monospace;font-size:13px}
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
let project = null, sel = null, envIdx = 0, query = '';
const esc = s => String(s ?? '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
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
  return '<table><thead><tr><th>Key</th><th>Value</th><th>Description</th></tr></thead><tbody>'+rows.map(r=>`<tr><td><code>${esc(r.key)}</code></td><td><code>${esc(resolve(r.value||''))}</code></td><td style="color:var(--muted)">${esc(r.description||'')}</td></tr>`).join('')+'</tbody></table>';
}
function render(){
  if(!project) return;
  const sb = document.getElementById('sidebar');
  let html = `<div class="tree-item ${sel?.type==='project'?'active':''}" onclick="sel={type:'project'};render()"><strong>${esc(project.doc.name)}</strong></div>`;
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
    let u = r.url||'';
    if(u && !/^https?:\/\//i.test(u) && !u.startsWith('{{') && project.doc.baseUrl) u = project.doc.baseUrl.replace(/\/+$/,'')+'/'+u.replace(/^\/+/,'');
    ct.innerHTML = `<h1>${esc(d.name)}</h1><p style="color:var(--muted)">${esc(d.description||'')}</p><div class="url-bar"><span class="badge ${esc(m)}">${esc(m)}</span><span>${esc(resolve(u))}</span></div><div class="section"><div class="section-title">Query Params</div>${kvTable(r.params)}</div><div class="section"><div class="section-title">Headers</div>${kvTable(r.headers)}</div>${r.body&&r.body.type!=='none'?`<div class="section"><div class="section-title">Body</div><pre>${esc(r.body.content||JSON.stringify(r.body,null,2))}</pre></div>`:''}`;
  }
}
function findF(list,id){ for(const f of list||[]){ if(f.relPath===id) return f; const sub=findF(f.folders,id); if(sub) return sub; } }
function findR(id){ for(const r of project.requests||[]) if(r.relPath===id) return r; function w(fs){ for(const f of fs||[]){ for(const r of f.requests||[]) if(r.relPath===id) return r; const s=w(f.folders); if(s) return s; } } return w(project.folders); }
window.selectFolder = id => { const f=findF(project.folders,id); if(f){ sel={type:'folder',id,data:f}; render(); } };
window.selectReq = id => { const r=findR(id); if(r){ sel={type:'request',id,data:r}; render(); } };
document.getElementById('searchInput').oninput = e => { query = e.target.value.trim().toLowerCase(); render(); };
fetch('/api/project').then(r=>r.json()).then(d=>{
  project = d;
  document.getElementById('projTitle').textContent = d.doc.name || 'Testrix Local Share';
  sel = {type:'project'};
  const es = document.getElementById('envSelect');
  if((d.environments||[]).length){ es.style.display='inline-block'; es.innerHTML=d.environments.map((e,i)=>`<option value="${i}">${esc(e.name)}</option>`).join(''); es.onchange=()=>{ envIdx=Number(es.value); render(); }; }
  render();
});
</script>
</body>
</html>"#;
