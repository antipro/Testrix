const state = {
  user: null,
  requireEmailVerification: true,
  requireEula: true,
  projects: [],
  currentProject: null,
  selectedNode: null, // { type: 'project' | 'folder' | 'request', data }
  selectedEnvIndex: 0,
  collapsedFolders: new Set(),
  searchQuery: '',
  codeTarget: 'curl',
};

function esc(str) {
  return String(str ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function methodBadge(method) {
  const m = String(method || 'GET').toUpperCase();
  const cls = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].includes(m) ? `method-${m}` : 'method-OTHER';
  return `<span class="method-badge ${cls}">${esc(m)}</span>`;
}

async function api(url, opts = {}) {
  const res = await fetch(url, {
    headers: { 'Content-Type': 'application/json', ...(opts.headers || {}) },
    ...opts,
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    const err = new Error(data.error || `Request failed (${res.status})`);
    err.status = res.status;
    err.accessTokenRequired = Boolean(data.accessTokenRequired);
    throw err;
  }
  return data;
}

function navigate(path) {
  history.pushState({}, '', path);
  renderRoute();
}

window.addEventListener('popstate', () => renderRoute());

document.addEventListener('click', (e) => {
  const link = e.target.closest('a[data-nav]');
  if (link) {
    e.preventDefault();
    navigate(link.getAttribute('data-nav'));
  }
});

async function loadCurrentUser() {
  try {
    const cfg = await api('/api/auth/config');
    state.requireEmailVerification = cfg.requireEmailVerification !== false;
    state.requireEula = cfg.requireEula !== false;
  } catch {}
  try {
    const data = await api('/api/auth/me');
    state.user = data.user;
  } catch {
    state.user = null;
  }
  renderAuthBar();
}

function renderAuthBar() {
  const bar = document.getElementById('authBar');
  if (!bar) return;
  if (state.user) {
    bar.innerHTML = `
      <span class="pill">${esc(state.user.email)}</span>
      <a href="/sync" class="btn btn-sm" data-nav="/sync">Git Sync</a>
      <a href="/dashboard" class="btn btn-sm" data-nav="/dashboard">Dashboard</a>
      <button class="btn btn-sm" id="logoutBtn">Logout</button>
    `;
    document.getElementById('logoutBtn').onclick = async () => {
      await api('/api/auth/logout', { method: 'POST' });
      state.user = null;
      renderAuthBar();
      navigate('/');
    };
  } else {
    bar.innerHTML = `
      <a href="/login" class="btn btn-sm" data-nav="/login">Login</a>
      <a href="/register" class="btn btn-sm btn-primary" data-nav="/register">Register</a>
    `;
  }
}

function getEnvVarsMap() {
  const proj = state.currentProject;
  const map = new Map();
  if (!proj) return map;
  for (const v of proj.doc?.variables || []) {
    if (v.enabled !== false && v.key) map.set(v.key, v.value ?? '');
  }
  const envs = proj.environments || [];
  const env = envs[state.selectedEnvIndex];
  if (env) {
    for (const v of env.variables || []) {
      if (v.enabled !== false && v.key) map.set(v.key, v.secret ? '••••••••' : (v.value ?? ''));
    }
  }
  return map;
}

function resolveVars(str) {
  if (!str) return '';
  const vars = getEnvVarsMap();
  return String(str).replace(/\{\{([^}]+)\}\}/g, (match, key) => {
    const k = key.trim();
    return vars.has(k) ? vars.get(k) : match;
  });
}

function renderKvTable(rows, emptyText = 'None') {
  if (!rows || rows.length === 0) {
    return `<div style="color:var(--muted);font-size:13px;">${esc(emptyText)}</div>`;
  }
  return `
    <table class="kv-table">
      <thead><tr><th>Key</th><th>Value</th><th>Description</th></tr></thead>
      <tbody>
        ${rows
          .map(
            (r) => `
          <tr class="${r.enabled === false ? 'kv-disabled' : ''}">
            <td><code>${esc(r.key)}</code></td>
            <td><code>${esc(resolveVars(r.value ?? ''))}</code></td>
            <td style="color:var(--muted)">${esc(r.description || '')}</td>
          </tr>`
          )
          .join('')}
      </tbody>
    </table>
  `;
}

function renderAuthSummary(auth) {
  if (!auth || auth.type === 'none') return '<span class="pill">None</span>';
  if (auth.type === 'inherit') return '<span class="pill">Inherit from parent</span>';
  return `<pre class="code-block">${esc(JSON.stringify(auth, null, 2))}</pre>`;
}

function renderBodySection(body) {
  if (!body || body.type === 'none') {
    return `<div class="section"><div class="section-title">Body</div><div style="color:var(--muted);font-size:13px;">None</div></div>`;
  }
  if (body.type === 'json') {
    let text = body.content || '';
    try {
      text = JSON.stringify(JSON.parse(text), null, 2);
    } catch {}
    return `
      <div class="section">
        <div class="section-title">Body (JSON)</div>
        <pre class="code-block">${esc(text)}</pre>
      </div>
    `;
  }
  if (body.type === 'raw') {
    const fmt = body.rawFormat || body.format || 'raw';
    let text = body.content || '';
    if (fmt === 'json') {
      try {
        text = JSON.stringify(JSON.parse(text), null, 2);
      } catch {}
    }
    return `
      <div class="section">
        <div class="section-title">Body (${esc(fmt)})</div>
        <pre class="code-block">${esc(text)}</pre>
      </div>
    `;
  }
  if (body.type === 'formUrlEncoded' || body.type === 'urlencoded') {
    return `
      <div class="section">
        <div class="section-title">Body (x-www-form-urlencoded)</div>
        ${renderKvTable(body.fields)}
      </div>
    `;
  }
  if (body.type === 'multipart') {
    return `
      <div class="section">
        <div class="section-title">Body (multipart/form-data)</div>
        ${renderKvTable((body.fields || []).map((f) => ({ key: f.key, value: f.value || f.filePath || '', description: f.fieldType || f.description || '', enabled: f.enabled })))}
      </div>
    `;
  }
  if (body.type === 'binary') {
    return `
      <div class="section">
        <div class="section-title">Body (Binary)</div>
        <div style="font-size:13px;"><code>${esc(body.filePath || '')}</code> ${body.contentType ? `<span class="pill">${esc(body.contentType)}</span>` : ''}</div>
      </div>
    `;
  }
  return `<div class="section"><div class="section-title">Body</div><pre class="code-block">${esc(JSON.stringify(body, null, 2))}</pre></div>`;
}

function buildUrl(reqDoc) {
  const base = state.currentProject?.doc?.baseUrl || '';
  const rawUrl = reqDoc?.request?.url || '';
  let combined = rawUrl;
  if (rawUrl && !/^https?:\/\//i.test(rawUrl) && !rawUrl.startsWith('{{') && base) {
    combined = `${base.replace(/\/+$/, '')}/${rawUrl.replace(/^\/+/, '')}`;
  }
  return resolveVars(combined);
}

function matchesSearch(req) {
  const q = state.searchQuery.trim().toLowerCase();
  if (!q) return true;
  const name = (req.doc?.name || '').toLowerCase();
  const url = (req.doc?.request?.url || '').toLowerCase();
  const method = (req.doc?.request?.method || '').toLowerCase();
  return name.includes(q) || url.includes(q) || method.includes(q);
}

function folderHasMatch(folder) {
  if ((folder.requests || []).some(matchesSearch)) return true;
  return (folder.folders || []).some(folderHasMatch);
}

function renderSidebarTree() {
  const proj = state.currentProject;
  if (!proj) return '';

  const isProjActive = state.selectedNode?.type === 'project';
  let html = `
    <div class="tree-item ${isProjActive ? 'active' : ''}" data-select-project="1">
      <span class="tree-caret">◆</span>
      <span class="tree-label"><strong>${esc(proj.doc?.name || proj.slug)}</strong></span>
    </div>
  `;

  function renderFolder(folder) {
    if (state.searchQuery && !folderHasMatch(folder)) return '';
    const key = folder.relPath;
    const collapsed = state.collapsedFolders.has(key) && !state.searchQuery;
    const isActive = state.selectedNode?.type === 'folder' && state.selectedNode.data.relPath === key;

    let inner = '';
    if (!collapsed) {
      for (const sub of folder.folders || []) inner += renderFolder(sub);
      for (const req of folder.requests || []) inner += renderRequest(req);
    }

    return `
      <div>
        <div class="tree-item ${isActive ? 'active' : ''}" data-select-folder="${esc(key)}">
          <span class="tree-caret" data-toggle-folder="${esc(key)}">${collapsed ? '▸' : '▾'}</span>
          <span class="tree-label">${esc(folder.doc?.name || key)}</span>
        </div>
        <div class="tree-folder-children">${inner}</div>
      </div>
    `;
  }

  function renderRequest(req) {
    if (!matchesSearch(req)) return '';
    const isActive = state.selectedNode?.type === 'request' && state.selectedNode.data.relPath === req.relPath;
    const method = req.doc?.request?.method || 'GET';
    return `
      <div class="tree-item ${isActive ? 'active' : ''}" data-select-request="${esc(req.relPath)}">
        ${methodBadge(method)}
        <span class="tree-label">${esc(req.doc?.name || req.relPath)}</span>
      </div>
    `;
  }

  for (const f of proj.folders || []) html += renderFolder(f);
  for (const r of proj.requests || []) html += renderRequest(r);
  return html;
}

function renderRightContent() {
  const proj = state.currentProject;
  const sel = state.selectedNode;
  if (!proj || !sel) {
    return `<div class="empty-state">Select a project or API endpoint from the left tree.</div>`;
  }

  if (sel.type === 'project') {
    const d = proj.doc || {};
    return `
      <div class="doc-header">
        <div>
          <h1 class="doc-title">${esc(d.name || proj.slug)}</h1>
          <div style="margin-top:6px;display:flex;gap:8px;">
            <span class="pill">${esc(d.apiVersion || 'testrix.io/v1')}</span>
            ${proj.meta?.ownerEmail ? `<span class="pill">By ${esc(proj.meta.ownerEmail)}</span>` : ''}
          </div>
        </div>
        <button class="btn btn-sm" id="copyShareLinkBtn">Copy Share URL</button>
      </div>
      ${d.description ? `<p class="doc-desc">${esc(d.description)}</p>` : ''}
      ${d.baseUrl ? `<div class="section"><div class="section-title">Base URL</div><div class="url-bar"><span class="url-text">${esc(resolveVars(d.baseUrl))}</span></div></div>` : ''}
      <div class="section">
        <div class="section-title">Authentication</div>
        ${renderAuthSummary(d.auth)}
      </div>
      <div class="section">
        <div class="section-title">Default Headers</div>
        ${renderKvTable(d.defaultHeaders)}
      </div>
      <div class="section">
        <div class="section-title">Project Variables</div>
        ${renderKvTable(d.variables)}
      </div>
    `;
  }

  if (sel.type === 'folder') {
    const f = sel.data;
    const d = f.doc || {};
    return `
      <div class="doc-header">
        <h1 class="doc-title">${esc(d.name || f.relPath)}</h1>
      </div>
      ${d.description ? `<p class="doc-desc">${esc(d.description)}</p>` : ''}
      <div class="section">
        <div class="section-title">Authentication</div>
        ${renderAuthSummary(d.auth)}
      </div>
      <div class="section">
        <div class="section-title">Default Headers</div>
        ${renderKvTable(d.defaultHeaders)}
      </div>
      <div class="section">
        <div class="section-title">Endpoints (${(f.requests || []).length})</div>
        ${(f.requests || []).length === 0 ? '<div style="color:var(--muted)">No requests in this folder.</div>' : `
          <table class="kv-table">
            <thead><tr><th>Method</th><th>Name</th><th>URL</th></tr></thead>
            <tbody>
              ${(f.requests || []).map((r) => `
                <tr style="cursor:pointer" data-select-request="${esc(r.relPath)}">
                  <td>${methodBadge(r.doc?.request?.method)}</td>
                  <td>${esc(r.doc?.name)}</td>
                  <td><code>${esc(r.doc?.request?.url)}</code></td>
                </tr>
              `).join('')}
            </tbody>
          </table>
        `}
      </div>
    `;
  }

  // Request view
  const req = sel.data;
  const d = req.doc || {};
  const r = d.request || {};
  const resolvedUrl = buildUrl(d);

  return `
    <div class="doc-header">
      <h1 class="doc-title">${esc(d.name || req.relPath)}</h1>
      <div style="display:flex;gap:8px;">
        <button class="btn btn-sm" id="copyReqLinkBtn">Copy Link</button>
        <button class="btn btn-sm" id="copyCurlBtn">Copy cURL</button>
      </div>
    </div>
    ${d.description ? `<p class="doc-desc">${esc(d.description)}</p>` : ''}
    <div class="url-bar">
      ${methodBadge(r.method)}
      <span class="url-text">${esc(resolvedUrl)}</span>
    </div>
    <div class="section">
      <div class="section-title">Query Parameters</div>
      ${renderKvTable(r.params)}
    </div>
    <div class="section">
      <div class="section-title">Headers</div>
      ${renderKvTable(r.headers)}
    </div>
    <div class="section">
      <div class="section-title">Authentication</div>
      ${renderAuthSummary(r.auth)}
    </div>
    ${renderBodySection(r.body)}
    ${renderResponseMetaSection(r.bodyMeta, 'Body Meta Info')}
    ${renderCodeSection(d)}
    ${renderResponseMetaSection(r.responseMeta, 'Response Meta Info')}
    ${renderResponseExamplesSection(r.responseExamples)}
  `;
}

const CODEGEN_TARGETS = [
  { id: 'curl', label: 'cURL' },
  { id: 'curl-win', label: 'cURL (Win)' },
  { id: 'wget', label: 'wget' },
  { id: 'powershell', label: 'PowerShell' },
];

function escapePosixSingle(s) {
  return String(s).replace(/'/g, "'\\''");
}

function escapeCmdDouble(s) {
  return String(s).replace(/"/g, '\\"').replace(/\r/g, '').replace(/\n/g, '\\n');
}

function escapePsSingle(s) {
  return String(s).replace(/'/g, "''");
}

function escapePsDouble(s) {
  return String(s).replace(/`/g, '``').replace(/"/g, '`"').replace(/\$/g, '`$');
}

function prepareRequestForCodegen(reqDoc) {
  const r = reqDoc?.request || {};
  const method = (r.method || 'GET').toUpperCase();
  let url = buildUrl(reqDoc);

  const enabledParams = (r.params || []).filter((p) => p.enabled !== false && p.key);
  if (enabledParams.length > 0) {
    const qs = enabledParams
      .map((p) => `${encodeURIComponent(resolveVars(p.key))}=${encodeURIComponent(resolveVars(p.value || ''))}`)
      .join('&');
    url += (url.includes('?') ? '&' : '?') + qs;
  }

  const headers = (r.headers || [])
    .filter((h) => h.enabled !== false && h.key)
    .map((h) => ({ name: resolveVars(h.key), value: resolveVars(h.value || '') }));

  const hasContentType = headers.some((h) => h.name.toLowerCase() === 'content-type');
  const body = r.body || { type: 'none' };
  if (!hasContentType) {
    if (body.type === 'json') {
      headers.push({ name: 'Content-Type', value: 'application/json' });
    } else if (body.type === 'raw') {
      const fmt = body.rawFormat || body.format || 'text';
      if (fmt === 'json') headers.push({ name: 'Content-Type', value: 'application/json' });
      else if (fmt === 'xml') headers.push({ name: 'Content-Type', value: 'application/xml' });
      else if (fmt === 'html') headers.push({ name: 'Content-Type', value: 'text/html' });
      else headers.push({ name: 'Content-Type', value: 'text/plain' });
    } else if (body.type === 'formUrlEncoded') {
      headers.push({ name: 'Content-Type', value: 'application/x-www-form-urlencoded' });
    } else if (body.type === 'binary' && body.contentType) {
      headers.push({ name: 'Content-Type', value: resolveVars(body.contentType) });
    }
  }

  return { method, url, headers, body };
}

function generateRequestCode(reqDoc, target) {
  const req = prepareRequestForCodegen(reqDoc);
  const { method, url, headers, body } = req;

  if (target === 'curl-win') {
    const lines = [`curl.exe -X ${method} "${escapeCmdDouble(url)}"`];
    for (const h of headers) {
      lines.push(`  -H "${escapeCmdDouble(h.name)}: ${escapeCmdDouble(h.value)}"`);
    }
    if (body.type === 'json' || body.type === 'raw') {
      lines.push(`  --data-raw "${escapeCmdDouble(resolveVars(body.content || ''))}"`);
    } else if (body.type === 'formUrlEncoded') {
      for (const f of body.fields || []) {
        if (f.enabled === false || !f.key) continue;
        lines.push(`  --data-urlencode "${escapeCmdDouble(resolveVars(f.key))}=${escapeCmdDouble(resolveVars(f.value || ''))}"`);
      }
    } else if (body.type === 'multipart') {
      for (const f of body.fields || []) {
        if (f.enabled === false || !f.key) continue;
        const k = escapeCmdDouble(resolveVars(f.key));
        const v = escapeCmdDouble(resolveVars(f.value || ''));
        lines.push(f.fieldType === 'file' ? `  -F "${k}=@${v}"` : `  -F "${k}=${v}"`);
      }
    } else if (body.type === 'binary' && body.filePath) {
      lines.push(`  --data-binary "@${escapeCmdDouble(resolveVars(body.filePath))}"`);
    }
    return lines.join(' ^\n');
  }

  if (target === 'wget') {
    const lines = [`wget --method=${method}`];
    for (const h of headers) {
      lines.push(`  --header='${escapePosixSingle(h.name)}: ${escapePosixSingle(h.value)}'`);
    }
    if (body.type === 'json' || body.type === 'raw') {
      lines.push(`  --body-data='${escapePosixSingle(resolveVars(body.content || ''))}'`);
    } else if (body.type === 'formUrlEncoded') {
      const encoded = (body.fields || [])
        .filter((f) => f.enabled !== false && f.key)
        .map((f) => `${encodeURIComponent(resolveVars(f.key))}=${encodeURIComponent(resolveVars(f.value || ''))}`)
        .join('&');
      lines.push(`  --body-data='${escapePosixSingle(encoded)}'`);
    } else if (body.type === 'multipart') {
      lines.push('  # Note: wget does not natively build multipart/form-data; use cURL');
    } else if (body.type === 'binary' && body.filePath) {
      lines.push(`  --body-file='${escapePosixSingle(resolveVars(body.filePath))}'`);
    }
    lines.push(`  -qO- '${escapePosixSingle(url)}'`);
    return lines.join(' \\\n');
  }

  if (target === 'powershell') {
    const out = [];
    let contentType = null;
    const headerLines = [];
    for (const h of headers) {
      if (h.name.toLowerCase() === 'content-type') {
        contentType = h.value;
      } else {
        headerLines.push(`    "${escapePsDouble(h.name)}" = "${escapePsDouble(h.value)}"`);
      }
    }
    const hasHeaders = headerLines.length > 0;
    if (hasHeaders) {
      out.push('$headers = @{', ...headerLines, '}');
    }

    let hasBody = false;
    if (body.type === 'json' || body.type === 'raw') {
      hasBody = true;
      const content = resolveVars(body.content || '');
      if (content.includes('\n')) {
        out.push(`$body = @'\n${content}\n'@`);
      } else {
        out.push(`$body = '${escapePsSingle(content)}'`);
      }
    } else if (body.type === 'formUrlEncoded') {
      hasBody = true;
      out.push('$body = @{');
      for (const f of body.fields || []) {
        if (f.enabled === false || !f.key) continue;
        out.push(`    "${escapePsDouble(resolveVars(f.key))}" = "${escapePsDouble(resolveVars(f.value || ''))}"`);
      }
      out.push('}');
    } else if (body.type === 'multipart') {
      hasBody = true;
      out.push('$body = @{');
      for (const f of body.fields || []) {
        if (f.enabled === false || !f.key) continue;
        const k = escapePsDouble(resolveVars(f.key));
        const v = resolveVars(f.value || '');
        if (f.fieldType === 'file') {
          out.push(`    "${k}" = Get-Item -Path '${escapePsSingle(v)}'`);
        } else {
          out.push(`    "${k}" = "${escapePsDouble(v)}"`);
        }
      }
      out.push('}');
    } else if (body.type === 'binary' && body.filePath) {
      hasBody = true;
      out.push(`$body = [System.IO.File]::ReadAllBytes('${escapePsSingle(resolveVars(body.filePath))}')`);
    }

    let invoke = `$response = Invoke-WebRequest -Uri '${escapePsSingle(url)}' -Method ${method} -UseBasicParsing`;
    if (hasHeaders) invoke += ' -Headers $headers';
    if (contentType) invoke += ` -ContentType '${escapePsSingle(contentType)}'`;
    if (hasBody) invoke += ' -Body $body';
    out.push(invoke);
    return out.join('\n');
  }

  // Default: POSIX cURL
  const lines = [`curl -X ${method} '${escapePosixSingle(url)}'`];
  for (const h of headers) {
    lines.push(`  -H '${escapePosixSingle(h.name)}: ${escapePosixSingle(h.value)}'`);
  }
  if (body.type === 'json' || body.type === 'raw') {
    lines.push(`  --data-raw '${escapePosixSingle(resolveVars(body.content || ''))}'`);
  } else if (body.type === 'formUrlEncoded') {
    for (const f of body.fields || []) {
      if (f.enabled === false || !f.key) continue;
      lines.push(`  --data-urlencode '${escapePosixSingle(resolveVars(f.key))}=${escapePosixSingle(resolveVars(f.value || ''))}'`);
    }
  } else if (body.type === 'multipart') {
    for (const f of body.fields || []) {
      if (f.enabled === false || !f.key) continue;
      const k = escapePosixSingle(resolveVars(f.key));
      const v = escapePosixSingle(resolveVars(f.value || ''));
      lines.push(f.fieldType === 'file' ? `  -F '${k}=@${v}'` : `  -F '${k}=${v}'`);
    }
  } else if (body.type === 'binary' && body.filePath) {
    lines.push(`  --data-binary '@${escapePosixSingle(resolveVars(body.filePath))}'`);
  }
  return lines.join(' \\\n');
}

function renderCodeSection(reqDoc) {
  const snippet = generateRequestCode(reqDoc, state.codeTarget);
  return `
    <div class="section">
      <div class="code-section-header">
        <div class="section-title" style="margin:0">Code Snippet</div>
        <div class="code-tabs">
          ${CODEGEN_TARGETS.map(
            (t) => `<button class="btn btn-sm code-tab-btn ${state.codeTarget === t.id ? 'active' : ''}" data-code-target="${t.id}">${esc(t.label)}</button>`
          ).join('')}
          <button class="btn btn-sm" id="copyCodeBtn">Copy</button>
        </div>
      </div>
      <pre class="code-block" id="codeSnippetBlock">${esc(snippet)}</pre>
    </div>
  `;
}

function renderResponseMetaSection(meta, title = 'Response Meta Info') {
  if (!Array.isArray(meta) || meta.length === 0) return '';
  const rows = [];
  function walk(fields, depth) {
    for (const f of fields || []) {
      const indentPx = depth * 18;
      const prefix = depth > 0 ? '<span class="meta-branch">└─</span> ' : '';
      rows.push(`
        <tr>
          <td>
            <div class="meta-field-cell" style="padding-left:${indentPx}px">
              ${prefix}<code>${esc(f.name)}</code>
            </div>
          </td>
          <td><span class="pill">${esc(f.type || 'any')}</span></td>
          <td><code class="meta-example">${esc(f.example || '—')}</code></td>
          <td style="color:var(--muted)">${esc(f.description || '')}</td>
        </tr>
      `);
      if (Array.isArray(f.children) && f.children.length > 0) {
        walk(f.children, depth + 1);
      }
    }
  }
  walk(meta, 0);
  return `
    <div class="section">
      <div class="section-title">${esc(title)}</div>
      <table class="kv-table">
        <thead><tr><th>Field</th><th>Type</th><th>Example</th><th>Description</th></tr></thead>
        <tbody>${rows.join('')}</tbody>
      </table>
    </div>
  `;
}

function statusBadgeHtml(code, reason) {
  const n = Number(code || 0);
  let cls = 'status-other';
  if (n >= 200 && n < 300) cls = 'status-2xx';
  else if (n >= 300 && n < 400) cls = 'status-3xx';
  else if (n >= 400) cls = 'status-err';
  const label = reason ? `${n} ${reason}` : `${n}`;
  return `<span class="status-badge ${cls}">${esc(label)}</span>`;
}

function formatExampleBody(body, contentType) {
  const raw = String(body ?? '');
  const ct = String(contentType || '').toLowerCase();
  if (ct.includes('json') || raw.trim().startsWith('{') || raw.trim().startsWith('[')) {
    try {
      return JSON.stringify(JSON.parse(raw), null, 2);
    } catch {}
  }
  return raw;
}

function renderResponseExamplesSection(examples) {
  if (!Array.isArray(examples) || examples.length === 0) return '';
  return `
    <div class="section">
      <div class="section-title">Response Examples (${examples.length})</div>
      <div class="examples-list">
        ${examples
          .map(
            (ex) => `
          <div class="example-card">
            <div class="example-header">
              ${statusBadgeHtml(ex.statusCode, ex.reason)}
              <span class="example-name">${esc(ex.name || 'Example')}</span>
              ${ex.contentType ? `<span class="pill">${esc(ex.contentType)}</span>` : ''}
            </div>
            <pre class="code-block">${esc(formatExampleBody(ex.body, ex.contentType))}</pre>
          </div>`
          )
          .join('')}
      </div>
    </div>
  `;
}

function findFolderByPath(folders, relPath) {
  for (const f of folders || []) {
    if (f.relPath === relPath) return f;
    const sub = findFolderByPath(f.folders, relPath);
    if (sub) return sub;
  }
  return null;
}

function findRequestByPath(proj, relPath) {
  for (const r of proj.requests || []) {
    if (r.relPath === relPath) return r;
  }
  function walk(folders) {
    for (const f of folders || []) {
      for (const r of f.requests || []) {
        if (r.relPath === relPath) return r;
      }
      const sub = walk(f.folders);
      if (sub) return sub;
    }
    return null;
  }
  return walk(proj.folders);
}

function findRequestByTarget(proj, target) {
  if (!proj || !target) return null;
  const normalized = String(target).replace(/^\/+/, '');
  const byPath = findRequestByPath(proj, normalized);
  if (byPath) return byPath;

  for (const r of proj.requests || []) {
    if (r.doc?.id === normalized) return r;
  }
  function walk(folders) {
    for (const f of folders || []) {
      for (const r of f.requests || []) {
        if (r.doc?.id === normalized) return r;
      }
      const sub = walk(f.folders);
      if (sub) return sub;
    }
    return null;
  }
  return walk(proj.folders);
}

function buildRequestSharePath(slug, relPath) {
  const encodedRel = String(relPath || '')
    .split('/')
    .filter(Boolean)
    .map(encodeURIComponent)
    .join('/');
  return `/s/${encodeURIComponent(slug)}/${encodedRel}`;
}

function renderViewerLayout() {
  const root = document.getElementById('appRoot');
  if (!state.currentProject) {
    root.innerHTML = `<div class="empty-state" style="width:100%">No YAML projects found. Upload a project from Testrix App or place a project folder in <code>YAML_DIR</code>.</div>`;
    return;
  }

  root.innerHTML = `
    <aside class="sidebar" id="sidebarTree">${renderSidebarTree()}</aside>
    <section class="content" id="docContent">${renderRightContent()}</section>
  `;

  root.querySelectorAll('[data-toggle-folder]').forEach((el) => {
    el.onclick = (e) => {
      e.stopPropagation();
      const key = el.getAttribute('data-toggle-folder');
      if (state.collapsedFolders.has(key)) state.collapsedFolders.delete(key);
      else state.collapsedFolders.add(key);
      renderViewerLayout();
    };
  });

  root.querySelectorAll('[data-select-project]').forEach((el) => {
    el.onclick = () => {
      state.selectedNode = { type: 'project', data: state.currentProject };
      if (state.currentProject?.slug) {
        history.pushState({}, '', `/s/${encodeURIComponent(state.currentProject.slug)}`);
      }
      renderViewerLayout();
    };
  });

  root.querySelectorAll('[data-select-folder]').forEach((el) => {
    el.onclick = () => {
      const f = findFolderByPath(state.currentProject.folders, el.getAttribute('data-select-folder'));
      if (f) {
        state.selectedNode = { type: 'folder', data: f };
        renderViewerLayout();
      }
    };
  });

  root.querySelectorAll('[data-select-request]').forEach((el) => {
    el.onclick = () => {
      const r = findRequestByPath(state.currentProject, el.getAttribute('data-select-request'));
      if (r) {
        state.selectedNode = { type: 'request', data: r };
        if (state.currentProject?.slug) {
          history.pushState({}, '', buildRequestSharePath(state.currentProject.slug, r.relPath));
        }
        renderViewerLayout();
      }
    };
  });

  const copyReqLinkBtn = document.getElementById('copyReqLinkBtn');
  if (copyReqLinkBtn && state.selectedNode?.type === 'request' && state.currentProject?.slug) {
    copyReqLinkBtn.onclick = () => {
      const url = `${location.origin}${buildRequestSharePath(state.currentProject.slug, state.selectedNode.data.relPath)}`;
      navigator.clipboard?.writeText(url);
      copyReqLinkBtn.textContent = 'Copied!';
      setTimeout(() => (copyReqLinkBtn.textContent = 'Copy Link'), 1500);
    };
  }

  const copyShareBtn = document.getElementById('copyShareLinkBtn');
  if (copyShareBtn) {
    copyShareBtn.onclick = () => {
      const url = `${location.origin}/s/${state.currentProject.slug}`;
      navigator.clipboard?.writeText(url);
      copyShareBtn.textContent = 'Copied!';
      setTimeout(() => (copyShareBtn.textContent = 'Copy Share URL'), 1500);
    };
  }

  const copyCurlBtn = document.getElementById('copyCurlBtn');
  if (copyCurlBtn && state.selectedNode?.type === 'request') {
    copyCurlBtn.onclick = () => {
      const code = generateRequestCode(state.selectedNode.data.doc, 'curl');
      navigator.clipboard?.writeText(code);
      copyCurlBtn.textContent = 'Copied!';
      setTimeout(() => (copyCurlBtn.textContent = 'Copy cURL'), 1500);
    };
  }

  document.querySelectorAll('.code-tab-btn[data-code-target]').forEach((btn) => {
    btn.onclick = () => {
      state.codeTarget = btn.getAttribute('data-code-target') || 'curl';
      if (state.selectedNode?.type === 'request') {
        const block = document.getElementById('codeSnippetBlock');
        if (block) {
          block.textContent = generateRequestCode(state.selectedNode.data.doc, state.codeTarget);
        }
        document.querySelectorAll('.code-tab-btn[data-code-target]').forEach((b) => {
          b.classList.toggle('active', b.getAttribute('data-code-target') === state.codeTarget);
        });
      }
    };
  });

  const copyCodeBtn = document.getElementById('copyCodeBtn');
  if (copyCodeBtn && state.selectedNode?.type === 'request') {
    copyCodeBtn.onclick = () => {
      const code = generateRequestCode(state.selectedNode.data.doc, state.codeTarget);
      navigator.clipboard?.writeText(code);
      copyCodeBtn.textContent = 'Copied!';
      setTimeout(() => (copyCodeBtn.textContent = 'Copy'), 1500);
    };
  }
}

function renderAccessTokenPrompt(slug, reqTarget = null) {
  const root = document.getElementById('appRoot');
  root.innerHTML = `
    <div class="card-container">
      <div class="card auth-card">
        <h2 style="margin-top:0">Access Token Required</h2>
        <p style="color:var(--muted);font-size:13px;">This shared API project is protected. Enter the access token to view its documentation.</p>
        <div id="tokenError" class="error-banner" style="display:none"></div>
        <form id="tokenForm">
          <div class="form-group">
            <label>Access Token</label>
            <input class="input" type="password" name="accessToken" required placeholder="Enter access token" />
          </div>
          <button type="submit" class="btn btn-primary" style="width:100%;justify-content:center">Unlock</button>
        </form>
      </div>
    </div>
  `;
  document.getElementById('tokenForm').onsubmit = async (e) => {
    e.preventDefault();
    const errEl = document.getElementById('tokenError');
    errEl.style.display = 'none';
    const form = new FormData(e.target);
    try {
      await api(`/api/public/projects/${encodeURIComponent(slug)}/unlock`, {
        method: 'POST',
        body: JSON.stringify({ accessToken: form.get('accessToken') }),
      });
      await loadProject(slug, reqTarget);
    } catch (err) {
      errEl.textContent = err.message;
      errEl.style.display = 'block';
    }
  };
}

function setTopbarViewerMode(isViewer) {
  const projSel = document.getElementById('projectSelect');
  const envSel = document.getElementById('envSelect');
  const searchInput = document.getElementById('searchInput');
  if (projSel) projSel.style.display = 'none';
  if (envSel && !isViewer) envSel.style.display = 'none';
  if (searchInput) searchInput.style.display = isViewer ? 'block' : 'none';
}

function renderIndexPage() {
  setTopbarViewerMode(false);
  state.currentProject = null;
  const root = document.getElementById('appRoot');
  root.innerHTML = `
    <div class="card-container">
      <div class="card" style="max-width:560px;margin:48px auto;padding:28px;">
        <h1 style="margin:0 0 8px;font-size:24px;">Testrix Share</h1>
        <p style="margin:0 0 22px;color:var(--muted);">
          Share interactive API documentation from your Testrix YAML projects.
          Projects are only accessible with a direct share link.
        </p>

        <form id="openLinkForm" style="display:flex;gap:8px;margin-bottom:20px;">
          <input
            class="input"
            name="shareLink"
            type="text"
            placeholder="Paste a share link or slug (e.g. /s/my-api)"
            style="flex:1;"
            required
          />
          <button type="submit" class="btn btn-primary">Open</button>
        </form>

        <div style="display:flex;gap:10px;flex-wrap:wrap;padding-top:16px;border-top:1px solid var(--border);">
          ${
            state.user
              ? `<a href="/dashboard" class="btn btn-primary" data-nav="/dashboard">Go to Dashboard</a>`
              : `<a href="/login" class="btn btn-primary" data-nav="/login">Sign In</a>
                 <a href="/register" class="btn" data-nav="/register">Create Account</a>`
          }
        </div>
      </div>
    </div>
  `;

  document.getElementById('openLinkForm').onsubmit = (e) => {
    e.preventDefault();
    const raw = String(new FormData(e.target).get('shareLink') || '').trim();
    if (!raw) return;
    let targetPath = raw;
    try {
      if (/^https?:\/\//i.test(raw)) {
        const u = new URL(raw);
        targetPath = u.pathname + u.search;
      }
    } catch {}
    if (!targetPath.startsWith('/s/')) {
      const cleaned = targetPath.replace(/^\/+/, '');
      targetPath = `/s/${encodeURIComponent(cleaned)}`;
    }
    navigate(targetPath);
  };
}

function renderProjectError(message) {
  setTopbarViewerMode(false);
  state.currentProject = null;
  const root = document.getElementById('appRoot');
  root.innerHTML = `
    <div class="card-container">
      <div class="card auth-card" style="text-align:center;">
        <h2 style="margin-top:0;">Unable to Open Project</h2>
        <p style="color:var(--muted);margin-bottom:18px;">${esc(message)}</p>
        <a href="/" class="btn btn-primary" data-nav="/">Back to Home</a>
      </div>
    </div>
  `;
}

async function loadProject(slug, reqTarget = null) {
  setTopbarViewerMode(true);
  let data;
  try {
    data = await api(`/api/public/projects/${encodeURIComponent(slug)}`);
  } catch (err) {
    if (err.accessTokenRequired) {
      renderAccessTokenPrompt(slug, reqTarget);
      return;
    }
    renderProjectError(err.message || 'Project not found');
    return;
  }
  state.currentProject = data.project;
  const matchedReq = reqTarget ? findRequestByTarget(data.project, reqTarget) : null;
  state.selectedNode = matchedReq
    ? { type: 'request', data: matchedReq }
    : { type: 'project', data: data.project };
  state.selectedEnvIndex = 0;

  const envSel = document.getElementById('envSelect');
  const envs = data.project.environments || [];
  if (envs.length > 0) {
    envSel.style.display = 'inline-block';
    envSel.innerHTML = envs.map((e, idx) => `<option value="${idx}">${esc(e.name || e.fileName)}</option>`).join('');
    envSel.onchange = () => {
      state.selectedEnvIndex = Number(envSel.value);
      renderViewerLayout();
    };
  } else {
    envSel.style.display = 'none';
  }
  renderViewerLayout();
}

function renderAuthForm(mode) {
  const isRegister = mode === 'register';
  const needCode = isRegister && state.requireEmailVerification;
  const needEula = isRegister && state.requireEula;
  const root = document.getElementById('appRoot');
  root.innerHTML = `
    <div class="card-container">
      <div class="card auth-card">
        <h2 style="margin-top:0">${isRegister ? 'Create Account' : 'Sign In'}</h2>
        <div id="authError" class="error-banner" style="display:none"></div>
        <div id="authInfo" style="display:none;background:rgba(34,197,94,0.12);border:1px solid rgba(34,197,94,0.35);color:#86efac;padding:8px 12px;border-radius:6px;margin-bottom:14px;font-size:13px;"></div>
        <form id="authForm">
          <div class="form-group">
            <label>Email</label>
            <div style="display:flex;gap:8px;">
              <input class="input" style="flex:1" type="email" name="email" id="authEmailInput" required placeholder="you@example.com" />
              ${needCode ? '<button type="button" class="btn btn-sm" id="sendCodeBtn">Send Code</button>' : ''}
            </div>
          </div>
          ${needCode ? `
            <div class="form-group">
              <label>Verification Code</label>
              <input class="input" type="text" name="code" required placeholder="6-digit code from email" />
            </div>
          ` : ''}
          <div class="form-group">
            <label>Password</label>
            <input class="input" type="password" name="password" required minlength="6" placeholder="••••••" />
          </div>
          ${isRegister ? `
            <div class="form-group">
              <label>Confirm Password</label>
              <input class="input" type="password" name="confirmPassword" required minlength="6" placeholder="••••••" />
            </div>
          ` : ''}
          ${needEula ? `
            <div class="form-group">
              <label>End User License Agreement</label>
              <div style="background:var(--surface-2);border:1px solid var(--border);border-radius:6px;padding:10px 12px;max-height:140px;overflow-y:auto;font-size:12px;color:var(--muted);line-height:1.5;margin-bottom:8px;">
                <p style="margin:0 0 6px;color:var(--text);font-weight:600;">End User License Agreement (EULA)</p>
                <p style="margin:0 0 6px;">1. <strong>Unencrypted Storage:</strong> All uploaded YAML files and project data are stored on the server in unencrypted form.</p>
                <p style="margin:0 0 6px;">2. <strong>No Sensitive Data:</strong> Do not upload any private, confidential, or sensitive content, including real passwords, API keys, access tokens, personal data, or proprietary secrets.</p>
                <p style="margin:0;">3. <strong>No Liability:</strong> By registering and using this service, you acknowledge that the service provider is not responsible or liable for any content leak, unauthorized access, or data exposure.</p>
              </div>
              <label style="display:flex;align-items:flex-start;gap:8px;font-size:12px;color:var(--text);cursor:pointer;">
                <input type="checkbox" name="acceptEula" required style="margin-top:2px;" />
                <span>I have read and agree to the End User License Agreement.</span>
              </label>
            </div>
          ` : ''}
          <button type="submit" class="btn btn-primary" style="width:100%;justify-content:center">
            ${isRegister ? 'Register' : 'Login'}
          </button>
        </form>
        <p style="margin-top:14px;font-size:13px;color:var(--muted)">
          ${isRegister ? 'Already have an account? <a href="/login" data-nav="/login">Login</a>' : 'Need an account? <a href="/register" data-nav="/register">Register</a>'}
        </p>
      </div>
    </div>
  `;

  if (needCode) {
    const sendBtn = document.getElementById('sendCodeBtn');
    sendBtn.onclick = async () => {
      const errEl = document.getElementById('authError');
      const infoEl = document.getElementById('authInfo');
      errEl.style.display = 'none';
      infoEl.style.display = 'none';
      const email = document.getElementById('authEmailInput').value.trim();
      if (!email) {
        errEl.textContent = 'Please enter your email first';
        errEl.style.display = 'block';
        return;
      }
      sendBtn.disabled = true;
      try {
        await api('/api/auth/send-code', {
          method: 'POST',
          body: JSON.stringify({ email }),
        });
        infoEl.textContent = 'Verification code sent! Check your mailbox.';
        infoEl.style.display = 'block';
        let secs = 60;
        const timer = setInterval(() => {
          secs -= 1;
          if (secs <= 0) {
            clearInterval(timer);
            sendBtn.disabled = false;
            sendBtn.textContent = 'Send Code';
          } else {
            sendBtn.textContent = `${secs}s`;
          }
        }, 1000);
      } catch (err) {
        sendBtn.disabled = false;
        errEl.textContent = err.message;
        errEl.style.display = 'block';
      }
    };
  }

  document.getElementById('authForm').onsubmit = async (e) => {
    e.preventDefault();
    const errEl = document.getElementById('authError');
    errEl.style.display = 'none';
    const form = new FormData(e.target);
    const email = form.get('email');
    const password = form.get('password');

    if (isRegister) {
      const confirmPassword = form.get('confirmPassword');
      if (password !== confirmPassword) {
        errEl.textContent = 'Passwords do not match';
        errEl.style.display = 'block';
        return;
      }
      if (needEula && !form.get('acceptEula')) {
        errEl.textContent = 'You must agree to the End User License Agreement to register';
        errEl.style.display = 'block';
        return;
      }
    }

    try {
      const payload = isRegister
        ? {
            email,
            password,
            confirmPassword: form.get('confirmPassword'),
            code: form.get('code'),
            acceptEula: Boolean(form.get('acceptEula')),
          }
        : { email, password };
      const data = await api(isRegister ? '/api/auth/register' : '/api/auth/login', {
        method: 'POST',
        body: JSON.stringify(payload),
      });
      state.user = data.user;
      renderAuthBar();
      navigate('/dashboard');
    } catch (err) {
      errEl.textContent = err.message;
      errEl.style.display = 'block';
    }
  };
}

async function renderDashboard() {
  if (!state.user) {
    navigate('/login');
    return;
  }
  const root = document.getElementById('appRoot');
  root.innerHTML = `<div class="card-container"><div class="card">Loading your shared projects...</div></div>`;

  const data = await api('/api/my/projects');
  const projects = data.projects || [];

  root.innerHTML = `
    <div class="card-container">
      <div class="doc-header">
        <h1 class="doc-title">My Shared Projects</h1>
        <a href="/" class="btn btn-sm" data-nav="/">Back to Viewer</a>
      </div>
      ${projects.length === 0 ? `
        <div class="card">
          <p style="color:var(--muted);margin:0">You have not shared any projects yet. Open a project in <strong>Testrix App</strong> → <strong>Share</strong> tab to upload its YAML files.</p>
        </div>
      ` : projects.map((p) => `
        <div class="card">
          <div style="display:flex;justify-content:space-between;align-items:flex-start;gap:12px;">
            <div>
              <h3 style="margin:0 0 6px">${esc(p.name)}</h3>
              <div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;">
                <span class="pill ${p.isPublic ? 'public' : 'private'}">${p.isPublic ? 'Public' : 'Private'}</span>
                <span class="pill">${p.visibleItems} / ${p.totalItems} items visible</span>
                <a href="/s/${esc(p.slug)}" data-nav="/s/${esc(p.slug)}" style="font-size:12px">${esc(p.shareUrl)}</a>
              </div>
            </div>
            <div style="display:flex;gap:8px;flex-wrap:wrap;">
              <button class="btn btn-sm" data-toggle-pub="${p.id}" data-cur="${p.isPublic ? 1 : 0}">
                ${p.isPublic ? 'Make Private' : 'Make Public'}
              </button>
              <button class="btn btn-sm" data-manage-items="${p.id}">Manage Items</button>
              <button class="btn btn-sm btn-danger" data-del-proj="${p.id}">Delete</button>
            </div>
          </div>
          <div id="items-panel-${p.id}" style="margin-top:14px;display:none;"></div>
        </div>
      `).join('')}
    </div>
  `;

  root.querySelectorAll('[data-toggle-pub]').forEach((btn) => {
    btn.onclick = async () => {
      const id = btn.getAttribute('data-toggle-pub');
      const cur = btn.getAttribute('data-cur') === '1';
      await api(`/api/my/projects/${id}`, {
        method: 'PATCH',
        body: JSON.stringify({ isPublic: !cur }),
      });
      renderDashboard();
    };
  });

  root.querySelectorAll('[data-del-proj]').forEach((btn) => {
    btn.onclick = async () => {
      if (!confirm('Delete this shared project and its uploaded YAML files?')) return;
      const id = btn.getAttribute('data-del-proj');
      await api(`/api/my/projects/${id}`, { method: 'DELETE' });
      renderDashboard();
    };
  });

  root.querySelectorAll('[data-manage-items]').forEach((btn) => {
    btn.onclick = async () => {
      const id = btn.getAttribute('data-manage-items');
      const panel = document.getElementById(`items-panel-${id}`);
      if (panel.style.display === 'block') {
        panel.style.display = 'none';
        return;
      }
      const res = await api(`/api/my/projects/${id}/items`);
      panel.style.display = 'block';
      panel.innerHTML = `
        <table class="kv-table">
          <thead><tr><th>Method</th><th>Name</th><th>Path</th><th>Visible</th></tr></thead>
          <tbody>
            ${(res.items || []).map((item) => `
              <tr>
                <td>${methodBadge(item.method)}</td>
                <td>${esc(item.name)}</td>
                <td style="color:var(--muted)"><code>${esc(item.relPath)}</code></td>
                <td>
                  <input type="checkbox" ${item.isVisible ? 'checked' : ''} data-item-toggle="${item.id}" data-proj="${id}" />
                </td>
              </tr>
            `).join('')}
          </tbody>
        </table>
      `;
      panel.querySelectorAll('[data-item-toggle]').forEach((cb) => {
        cb.onchange = async () => {
          await api(`/api/my/projects/${cb.getAttribute('data-proj')}/items/${cb.getAttribute('data-item-toggle')}`, {
            method: 'PATCH',
            body: JSON.stringify({ isVisible: cb.checked }),
          });
        };
      });
    };
  });
}

async function renderSyncPage() {
  if (!state.user) {
    return navigate('/login');
  }
  const root = document.getElementById('appRoot');
  root.className = 'page-shell';
  root.innerHTML = `<div class="card"><h2>Git Sync Repositories</h2><p class="muted">Loading repositories...</p></div>`;

  const data = await api('/api/sync/repos');
  const repos = data.repos || [];

  root.innerHTML = `
    <div class="card" style="max-width:1100px;margin:0 auto;">
      <div style="display:flex;justify-content:space-between;align-items:center;gap:12px;flex-wrap:wrap;margin-bottom:16px;">
        <div>
          <h2 style="margin:0;">Git Sync Repositories</h2>
          <p class="muted" style="margin:4px 0 0;">Independent pure-JS Git repositories for bidirectional merge sync with Testrix.</p>
        </div>
        <div style="display:flex;gap:8px;align-items:center;">
          <input id="newSyncRepoSlug" class="input" placeholder="workspace-slug" style="width:200px;" />
          <button id="createSyncRepoBtn" class="btn btn-primary btn-sm">Create Repo</button>
        </div>
      </div>
      <div id="syncReposList" style="display:flex;flex-wrap:wrap;gap:8px;margin-bottom:16px;">
        ${
          repos.length === 0
            ? '<span class="muted">No sync repositories yet. Create one above or sync from Testrix.</span>'
            : repos
                .map(
                  (r) =>
                    `<button class="btn btn-sm sync-repo-btn" data-slug="${esc(r.slug)}">${esc(r.name)} (${esc(r.slug)}) ${
                      r.headOid ? `· ${esc(r.headOid.slice(0, 7))}` : '· empty'
                    }</button>`
                )
                .join('')
        }
      </div>
      <div id="syncRepoDetail"></div>
    </div>
  `;

  document.getElementById('createSyncRepoBtn').onclick = async () => {
    const slug = document.getElementById('newSyncRepoSlug').value.trim() || 'workspace';
    await api('/api/sync/repos', {
      method: 'POST',
      body: JSON.stringify({ slug, name: slug }),
    });
    renderSyncPage();
  };

  async function openSyncRepo(slug) {
    const detail = document.getElementById('syncRepoDetail');
    detail.innerHTML = '<p class="muted">Loading repository state...</p>';
    const repoState = await api(`/api/sync/repos/${encodeURIComponent(slug)}/state`);
    const fileKeys = Object.keys(repoState.files || {}).sort();
    const commits = repoState.commits || [];
    const activeFile = fileKeys[0] || '';

    detail.innerHTML = `
      <div style="border-top:1px solid var(--border);padding-top:16px;">
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
          <div>
            <strong>Repository:</strong> <span class="pill">${esc(slug)}</span>
            ${repoState.headOid ? `<span class="muted" style="margin-left:8px;">HEAD ${esc(repoState.headOid.slice(0, 7))}</span>` : ''}
          </div>
          <button id="deleteSyncRepoBtn" class="btn btn-sm btn-danger">Delete Repo</button>
        </div>
        <div style="display:grid;grid-template-columns:260px 1fr;gap:16px;">
        <div>
          <h3 style="margin:0 0 8px;">Files (${fileKeys.length})</h3>
          <div id="syncFilesList" style="display:flex;flex-direction:column;gap:4px;max-height:320px;overflow:auto;">
            ${
              fileKeys.length === 0
                ? '<span class="muted">No files committed yet.</span>'
                : fileKeys
                    .map(
                      (fp) =>
                        `<button class="btn btn-sm sync-file-btn" data-file="${esc(fp)}" style="text-align:left;justify-content:flex-start;">${esc(fp)}</button>`
                    )
                    .join('')
            }
          </div>
          <h3 style="margin:16px 0 8px;">Recent Commits (${commits.length})</h3>
          <div style="display:flex;flex-direction:column;gap:6px;max-height:220px;overflow:auto;">
            ${commits
              .map(
                (c) => `
              <div style="padding:6px 8px;border:1px solid var(--border);border-radius:6px;font-size:12px;">
                <div><span class="pill">${esc(c.shortOid)}</span> ${esc(c.message)}</div>
                <div class="muted" style="margin-top:2px;">${esc(c.author)}</div>
              </div>`
              )
              .join('')}
          </div>
        </div>
        <div>
          <div style="display:flex;gap:8px;align-items:center;margin-bottom:8px;">
            <input id="syncEditorPath" class="input" value="${esc(activeFile)}" placeholder="path/to/file.request.yaml" style="flex:1;" />
            <input id="syncCommitMsg" class="input" placeholder="Commit message..." style="flex:1;" />
            <button id="syncSaveFileBtn" class="btn btn-primary btn-sm">Commit File</button>
          </div>
          <textarea id="syncEditorContent" style="width:100%;height:420px;font-family:monospace;font-size:12px;padding:10px;border-radius:8px;border:1px solid var(--border);background:var(--bg);color:var(--text);">${esc(
            activeFile ? repoState.files[activeFile] : ''
          )}</textarea>
          <div id="syncFileHistory" style="margin-top:12px;"></div>
        </div>
      </div>
      </div>
    `;

    document.getElementById('deleteSyncRepoBtn').onclick = async () => {
      if (!confirm(`Delete Git Sync repository "${slug}" from server? This cannot be undone.`)) {
        return;
      }
      await api(`/api/sync/repos/${encodeURIComponent(slug)}`, { method: 'DELETE' });
      renderSyncPage();
    };

    async function loadFileHistory(filepath) {
      const histDiv = document.getElementById('syncFileHistory');
      if (!filepath) {
        histDiv.innerHTML = '';
        return;
      }
      const logData = await api(
        `/api/sync/repos/${encodeURIComponent(slug)}/log?filepath=${encodeURIComponent(filepath)}&depth=20`
      );
      const list = logData.commits || [];
      histDiv.innerHTML = `
        <h4 style="margin:0 0 6px;">File History: ${esc(filepath)}</h4>
        <div style="display:flex;flex-wrap:wrap;gap:6px;">
          ${list
            .map(
              (c) =>
                `<button class="btn btn-sm sync-restore-btn" data-oid="${esc(c.oid)}" data-file="${esc(filepath)}" title="${esc(c.message)}">Restore ${esc(c.shortOid)}</button>`
            )
            .join('')}
        </div>
      `;
      histDiv.querySelectorAll('.sync-restore-btn').forEach((btn) => {
        btn.onclick = async () => {
          await api(`/api/sync/repos/${encodeURIComponent(slug)}/restore`, {
            method: 'POST',
            body: JSON.stringify({ oid: btn.dataset.oid, filepath: btn.dataset.file }),
          });
          openSyncRepo(slug);
        };
      });
    }

    detail.querySelectorAll('.sync-file-btn').forEach((btn) => {
      btn.onclick = () => {
        const fp = btn.dataset.file;
        document.getElementById('syncEditorPath').value = fp;
        document.getElementById('syncEditorContent').value = repoState.files[fp] || '';
        loadFileHistory(fp);
      };
    });

    document.getElementById('syncSaveFileBtn').onclick = async () => {
      const filepath = document.getElementById('syncEditorPath').value.trim();
      const content = document.getElementById('syncEditorContent').value;
      const message = document.getElementById('syncCommitMsg').value.trim() || `Update ${filepath}`;
      if (!filepath) return;
      await api(`/api/sync/repos/${encodeURIComponent(slug)}/file`, {
        method: 'POST',
        body: JSON.stringify({ filepath, content, message }),
      });
      openSyncRepo(slug);
    };

    if (activeFile) {
      loadFileHistory(activeFile);
    }
  }

  document.querySelectorAll('.sync-repo-btn').forEach((btn) => {
    btn.onclick = () => openSyncRepo(btn.dataset.slug);
  });
  if (repos.length > 0) {
    openSyncRepo(repos[0].slug);
  }
}

async function renderRoute() {
  const p = location.pathname;
  if (p === '/login') {
    setTopbarViewerMode(false);
    return renderAuthForm('login');
  }
  if (p === '/register') {
    setTopbarViewerMode(false);
    return renderAuthForm('register');
  }
  if (p === '/dashboard') {
    setTopbarViewerMode(false);
    return renderDashboard();
  }
  if (p === '/sync') {
    setTopbarViewerMode(false);
    return renderSyncPage();
  }

  const match = p.match(/^\/s\/([^/]+)(?:\/(.+))?$/);
  if (!match) {
    return renderIndexPage();
  }

  const slug = decodeURIComponent(match[1]);
  const reqTarget = match[2]
    ? match[2].split('/').map(decodeURIComponent).join('/')
    : new URLSearchParams(location.search).get('request');
  await loadProject(slug, reqTarget);
}

document.getElementById('searchInput').addEventListener('input', (e) => {
  state.searchQuery = e.target.value;
  if (location.pathname.startsWith('/s/')) {
    renderViewerLayout();
  }
});

(async () => {
  await loadCurrentUser();
  await renderRoute();
})();
