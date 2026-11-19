const state = {
  projects: [],
  currentProject: null,
  selectedNode: null,
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
    err.authRequired = Boolean(data.authRequired);
    err.accessTokenRequired = Boolean(data.accessTokenRequired);
    throw err;
  }
  return data;
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
          <div style="margin-top:6px;"><span class="pill">${esc(d.apiVersion || 'testrix.io/v1')}</span></div>
        </div>
      </div>
      ${d.description ? `<p class="doc-desc">${esc(d.description)}</p>` : ''}
      ${d.baseUrl ? `<div class="section"><div class="section-title">Base URL</div><div class="url-bar"><span class="url-text">${esc(resolveVars(d.baseUrl))}</span></div></div>` : ''}
      <div class="section"><div class="section-title">Authentication</div>${renderAuthSummary(d.auth)}</div>
      <div class="section"><div class="section-title">Default Headers</div>${renderKvTable(d.defaultHeaders)}</div>
      <div class="section"><div class="section-title">Project Variables</div>${renderKvTable(d.variables)}</div>
    `;
  }

  if (sel.type === 'folder') {
    const f = sel.data;
    const d = f.doc || {};
    return `
      <div class="doc-header"><h1 class="doc-title">${esc(d.name || f.relPath)}</h1></div>
      ${d.description ? `<p class="doc-desc">${esc(d.description)}</p>` : ''}
      <div class="section"><div class="section-title">Authentication</div>${renderAuthSummary(d.auth)}</div>
      <div class="section"><div class="section-title">Default Headers</div>${renderKvTable(d.defaultHeaders)}</div>
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
    <div class="url-bar">${methodBadge(r.method)}<span class="url-text">${esc(resolvedUrl)}</span></div>
    <div class="section"><div class="section-title">Query Parameters</div>${renderKvTable(r.params)}</div>
    <div class="section"><div class="section-title">Headers</div>${renderKvTable(r.headers)}</div>
    <div class="section"><div class="section-title">Authentication</div>${renderAuthSummary(r.auth)}</div>
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

function buildRequestQueryUrl(slug, relPath) {
  const params = new URLSearchParams();
  if (slug) params.set('project', slug);
  if (relPath) params.set('request', relPath);
  return `${location.pathname}?${params.toString()}`;
}

function renderViewerLayout() {
  const root = document.getElementById('appRoot');
  if (!state.currentProject) {
    root.innerHTML = `<div class="empty-state" style="width:100%">No YAML projects found in the configured R2 bucket.</div>`;
    return;
  }
  root.innerHTML = `
    <aside class="sidebar">${renderSidebarTree()}</aside>
    <section class="content">${renderRightContent()}</section>
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
        history.pushState({}, '', buildRequestQueryUrl(state.currentProject.slug, null));
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
          history.pushState({}, '', buildRequestQueryUrl(state.currentProject.slug, r.relPath));
        }
        renderViewerLayout();
      }
    };
  });

  const copyReqLinkBtn = document.getElementById('copyReqLinkBtn');
  if (copyReqLinkBtn && state.selectedNode?.type === 'request' && state.currentProject?.slug) {
    copyReqLinkBtn.onclick = () => {
      const url = `${location.origin}${buildRequestQueryUrl(state.currentProject.slug, state.selectedNode.data.relPath)}`;
      navigator.clipboard?.writeText(url);
      copyReqLinkBtn.textContent = 'Copied!';
      setTimeout(() => (copyReqLinkBtn.textContent = 'Copy Link'), 1500);
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

function renderPasscodePrompt() {
  const root = document.getElementById('appRoot');
  root.innerHTML = `
    <div class="card-container">
      <div class="card auth-card">
        <h2 style="margin-top:0">Private Access</h2>
        <p style="color:var(--muted);font-size:13px;">Enter the access passcode to view API documentation.</p>
        <div id="passError" class="error-banner" style="display:none"></div>
        <form id="passForm">
          <div class="form-group">
            <label>Passcode</label>
            <input class="input" type="password" name="passcode" required />
          </div>
          <button type="submit" class="btn btn-primary" style="width:100%;justify-content:center">Unlock</button>
        </form>
      </div>
    </div>
  `;
  document.getElementById('passForm').onsubmit = async (e) => {
    e.preventDefault();
    const errEl = document.getElementById('passError');
    errEl.style.display = 'none';
    const form = new FormData(e.target);
    try {
      await api('/api/auth', {
        method: 'POST',
        body: JSON.stringify({ passcode: form.get('passcode') }),
      });
      await window.addEventListener('popstate', () => {
  const searchParams = new URLSearchParams(location.search);
  const targetSlug = searchParams.get('project') || state.currentProject?.slug;
  const reqTarget = searchParams.get('request');
  if (targetSlug) {
    loadProject(targetSlug, reqTarget);
  }
});

init();
    } catch (err) {
      errEl.textContent = err.message;
      errEl.style.display = 'block';
    }
  };
}

function renderAccessTokenPrompt(slug, reqTarget = null) {
  const root = document.getElementById('appRoot');
  root.innerHTML = `
    <div class="card-container">
      <div class="card auth-card">
        <h2 style="margin-top:0">Access Token Required</h2>
        <p style="color:var(--muted);font-size:13px;">Enter the access token for this project to view its API documentation.</p>
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
      await api(`/api/projects/${encodeURIComponent(slug)}/unlock`, {
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

async function loadProject(slug, reqTarget = null) {
  let data;
  try {
    data = await api(`/api/projects/${encodeURIComponent(slug)}`);
  } catch (err) {
    if (err.accessTokenRequired) {
      renderAccessTokenPrompt(slug, reqTarget);
      return;
    }
    throw err;
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

async function init() {
  try {
    const data = await api('/api/projects');
    state.projects = data.projects || [];
    const projSel = document.getElementById('projectSelect');
    projSel.innerHTML = state.projects.length
      ? state.projects.map((p) => `<option value="${esc(p.slug)}">${esc(p.name)}</option>`).join('')
      : '<option value="">No projects</option>';

    const searchParams = new URLSearchParams(location.search);
    const pathMatch = location.pathname.match(/^\/s\/([^/]+)(?:\/(.+))?$/);
    const urlSlug = searchParams.get('project') || (pathMatch ? decodeURIComponent(pathMatch[1]) : null);
    const reqTarget =
      searchParams.get('request') ||
      (pathMatch && pathMatch[2] ? pathMatch[2].split('/').map(decodeURIComponent).join('/') : null);
    const targetSlug = urlSlug || state.projects[0]?.slug;
    if (targetSlug) {
      projSel.value = targetSlug;
      await loadProject(targetSlug, reqTarget);
    } else {
      state.currentProject = null;
      renderViewerLayout();
    }
    projSel.onchange = () => {
      if (projSel.value) {
        history.pushState({}, '', buildRequestQueryUrl(projSel.value, null));
        loadProject(projSel.value);
      }
    };
  } catch (err) {
    if (err.authRequired) {
      renderPasscodePrompt();
    } else {
      document.getElementById('appRoot').innerHTML = `<div class="empty-state" style="width:100%">${esc(err.message)}</div>`;
    }
  }
}

document.getElementById('searchInput').addEventListener('input', (e) => {
  state.searchQuery = e.target.value;
  renderViewerLayout();
});

window.addEventListener('popstate', () => {
  const searchParams = new URLSearchParams(location.search);
  const targetSlug = searchParams.get('project') || state.currentProject?.slug;
  const reqTarget = searchParams.get('request');
  if (targetSlug) {
    loadProject(targetSlug, reqTarget);
  }
});

init();
