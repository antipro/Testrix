const state = {
  user: null,
  projects: [],
  currentProject: null,
  selectedNode: null, // { type: 'project' | 'folder' | 'request', data }
  selectedEnvIndex: 0,
  collapsedFolders: new Set(),
  searchQuery: '',
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
  if (!body || body.type === 'none') return '';
  if (body.type === 'raw') {
    let text = body.content || '';
    if (body.format === 'json') {
      try {
        text = JSON.stringify(JSON.parse(text), null, 2);
      } catch {}
    }
    return `
      <div class="section">
        <div class="section-title">Body (${esc(body.format || 'raw')})</div>
        <pre class="code-block">${esc(text)}</pre>
      </div>
    `;
  }
  if (body.type === 'urlencoded') {
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
        ${renderKvTable((body.fields || []).map((f) => ({ key: f.key, value: f.value || f.filePath || '', description: f.fieldType, enabled: f.enabled })))}
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
      <button class="btn btn-sm" id="copyCurlBtn">Copy cURL</button>
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
        renderViewerLayout();
      }
    };
  });

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
      const d = state.selectedNode.data.doc;
      const r = d.request || {};
      const method = (r.method || 'GET').toUpperCase();
      const url = buildUrl(d);
      const headers = (r.headers || [])
        .filter((h) => h.enabled !== false && h.key)
        .map((h) => `-H "${h.key}: ${resolveVars(h.value || '')}"`)
        .join(' ');
      const curl = `curl -X ${method} "${url}" ${headers}`.trim();
      navigator.clipboard?.writeText(curl);
      copyCurlBtn.textContent = 'Copied!';
      setTimeout(() => (copyCurlBtn.textContent = 'Copy cURL'), 1500);
    };
  }
}

function renderAccessTokenPrompt(slug) {
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
      await loadProject(slug);
    } catch (err) {
      errEl.textContent = err.message;
      errEl.style.display = 'block';
    }
  };
}

async function loadProject(slug) {
  let data;
  try {
    data = await api(`/api/public/projects/${encodeURIComponent(slug)}`);
  } catch (err) {
    if (err.accessTokenRequired) {
      renderAccessTokenPrompt(slug);
      return;
    }
    throw err;
  }
  state.currentProject = data.project;
  state.selectedNode = { type: 'project', data: data.project };
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

async function loadProjectsList(preferredSlug = null) {
  const data = await api('/api/public/projects');
  state.projects = data.projects || [];

  const projSel = document.getElementById('projectSelect');
  projSel.innerHTML = state.projects.length
    ? state.projects.map((p) => `<option value="${esc(p.slug)}">${esc(p.name)}</option>`).join('')
    : '<option value="">No projects</option>';

  const targetSlug = preferredSlug || state.projects[0]?.slug;
  if (targetSlug) {
    projSel.value = targetSlug;
    await loadProject(targetSlug);
  } else {
    state.currentProject = null;
    renderViewerLayout();
  }

  projSel.onchange = () => {
    if (projSel.value) navigate(`/s/${projSel.value}`);
  };
}

function renderAuthForm(mode) {
  const isRegister = mode === 'register';
  const root = document.getElementById('appRoot');
  root.innerHTML = `
    <div class="card-container">
      <div class="card auth-card">
        <h2 style="margin-top:0">${isRegister ? 'Create Account' : 'Sign In'}</h2>
        <div id="authError" class="error-banner" style="display:none"></div>
        <form id="authForm">
          <div class="form-group">
            <label>Email</label>
            <input class="input" type="email" name="email" required placeholder="you@example.com" />
          </div>
          <div class="form-group">
            <label>Password</label>
            <input class="input" type="password" name="password" required minlength="6" placeholder="••••••" />
          </div>
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

  document.getElementById('authForm').onsubmit = async (e) => {
    e.preventDefault();
    const errEl = document.getElementById('authError');
    errEl.style.display = 'none';
    const form = new FormData(e.target);
    try {
      const data = await api(isRegister ? '/api/auth/register' : '/api/auth/login', {
        method: 'POST',
        body: JSON.stringify({
          email: form.get('email'),
          password: form.get('password'),
        }),
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

async function renderRoute() {
  const p = location.pathname;
  if (p === '/login') return renderAuthForm('login');
  if (p === '/register') return renderAuthForm('register');
  if (p === '/dashboard') return renderDashboard();

  const match = p.match(/^\/s\/([^/]+)/);
  const slug = match ? decodeURIComponent(match[1]) : null;
  await loadProjectsList(slug);
}

document.getElementById('searchInput').addEventListener('input', (e) => {
  state.searchQuery = e.target.value;
  if (location.pathname === '/' || location.pathname.startsWith('/s/')) {
    renderViewerLayout();
  }
});

(async () => {
  await loadCurrentUser();
  await renderRoute();
})();
