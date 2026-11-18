const COOKIE_NAME = 'testrix_cf_pass';

export function parseCookies(header) {
  const out = {};
  if (!header) return out;
  for (const part of header.split(';')) {
    const idx = part.indexOf('=');
    if (idx === -1) continue;
    out[part.slice(0, idx).trim()] = decodeURIComponent(part.slice(idx + 1).trim());
  }
  return out;
}

export async function getProjectAccessToken(bucket, slug) {
  const obj = await bucket.get(`${slug}/.testrix-access.json`);
  if (!obj) return '';
  try {
    const parsed = JSON.parse(await obj.text());
    return String(parsed?.accessToken || '').trim();
  } catch {
    return '';
  }
}

export function isAuthorized(request, env) {
  const passcode = env.ACCESS_PASSCODE;
  if (!passcode) return true;
  const auth = request.headers.get('Authorization') || '';
  if (auth.startsWith('Bearer ') && auth.slice(7).trim() === passcode) {
    return true;
  }
  const cookies = parseCookies(request.headers.get('Cookie'));
  return cookies[COOKIE_NAME] === passcode;
}

export function jsonResponse(data, status = 200, headers = {}) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json; charset=utf-8', ...headers },
  });
}

export async function listAllObjects(bucket, prefix = '') {
  const objects = [];
  let cursor = undefined;
  do {
    const listed = await bucket.list({ prefix, cursor, limit: 1000 });
    objects.push(...(listed.objects || []));
    cursor = listed.truncated ? listed.cursor : undefined;
  } while (cursor);
  return objects;
}

function unquoteScalar(raw) {
  const s = raw.trim();
  if (s === '' || s === 'null' || s === '~') return null;
  if (s === 'true') return true;
  if (s === 'false') return false;
  if (s === '{}') return {};
  if (s === '[]') return [];
  if (/^-?\d+(\.\d+)?$/.test(s)) return Number(s);
  if (s.startsWith('"') && s.endsWith('"') && s.length >= 2) {
    try {
      return JSON.parse(s);
    } catch {
      return s.slice(1, -1);
    }
  }
  if (s.startsWith("'") && s.endsWith("'") && s.length >= 2) {
    return s.slice(1, -1).replace(/''/g, "'");
  }
  return s;
}

function stripInlineComment(line) {
  let inSingle = false;
  let inDouble = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (ch === "'" && !inDouble) inSingle = !inSingle;
    else if (ch === '"' && !inSingle && line[i - 1] !== '\\') inDouble = !inDouble;
    else if (ch === '#' && !inSingle && !inDouble && (i === 0 || /\s/.test(line[i - 1]))) {
      return line.slice(0, i).trimEnd();
    }
  }
  return line.trimEnd();
}

function splitKeyValue(text) {
  let inSingle = false;
  let inDouble = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch === "'" && !inDouble) inSingle = !inSingle;
    else if (ch === '"' && !inSingle && text[i - 1] !== '\\') inDouble = !inDouble;
    else if (ch === ':' && !inSingle && !inDouble && (i + 1 === text.length || /\s/.test(text[i + 1]))) {
      return [text.slice(0, i).trim(), text.slice(i + 1).trim()];
    }
  }
  return null;
}

function parseYaml(text) {
  const rawLines = String(text || '').replace(/\r\n/g, '\n').split('\n');
  const lines = [];
  for (let i = 0; i < rawLines.length; i++) {
    const raw = rawLines[i];
    if (!raw.trim() || raw.trimStart().startsWith('#') || raw.trim() === '---') continue;
    const indent = raw.search(/\S/);
    lines.push({ indent, raw, text: stripInlineComment(raw.slice(indent)) });
  }

  let idx = 0;

  function parseBlockScalar(parentIndent, indicator) {
    const fold = indicator.startsWith('>');
    const chompStrip = indicator.endsWith('-');
    const chunks = [];
    let blockIndent = null;
    while (idx < lines.length) {
      const cur = lines[idx];
      if (cur.indent <= parentIndent) break;
      if (blockIndent === null) blockIndent = cur.indent;
      chunks.push(cur.raw.slice(Math.min(blockIndent, cur.raw.length)));
      idx++;
    }
    let out = fold ? chunks.join(' ') : chunks.join('\n');
    if (!chompStrip && out.length > 0 && !out.endsWith('\n')) out += '\n';
    return chompStrip ? out.replace(/\n+$/, '') : out;
  }

  function parseNode(minIndent) {
    if (idx >= lines.length) return null;
    if (lines[idx].text.startsWith('- ') || lines[idx].text === '-') {
      return parseSequence(lines[idx].indent);
    }
    return parseMapping(minIndent);
  }

  function parseSequence(seqIndent) {
    const arr = [];
    while (idx < lines.length) {
      const cur = lines[idx];
      if (cur.indent < seqIndent || (!cur.text.startsWith('- ') && cur.text !== '-')) break;
      if (cur.indent > seqIndent) break;

      const afterDash = cur.text === '-' ? '' : cur.text.slice(2).trim();
      idx++;

      if (!afterDash) {
        if (idx < lines.length && lines[idx].indent > seqIndent) {
          arr.push(parseNode(lines[idx].indent));
        } else {
          arr.push(null);
        }
        continue;
      }

      const kv = splitKeyValue(afterDash);
      if (kv) {
        const obj = {};
        const itemIndent = seqIndent + 2;
        assignKeyValue(obj, kv[0], kv[1], seqIndent);
        while (idx < lines.length && lines[idx].indent >= itemIndent) {
          if (lines[idx].indent === itemIndent && (lines[idx].text.startsWith('- ') || lines[idx].text === '-')) {
            break;
          }
          const nextKv = splitKeyValue(lines[idx].text);
          if (!nextKv) {
            idx++;
            continue;
          }
          const lineIndent = lines[idx].indent;
          idx++;
          assignKeyValue(obj, nextKv[0], nextKv[1], lineIndent);
        }
        arr.push(obj);
      } else {
        arr.push(unquoteScalar(afterDash));
      }
    }
    return arr;
  }

  function assignKeyValue(obj, rawKey, valPart, keyIndent) {
    const key = String(unquoteScalar(rawKey) ?? rawKey);
    if (valPart === '|' || valPart === '|-' || valPart === '>' || valPart === '>-') {
      obj[key] = parseBlockScalar(keyIndent, valPart);
    } else if (valPart === '') {
      if (idx < lines.length && lines[idx].indent > keyIndent) {
        obj[key] = parseNode(lines[idx].indent);
      } else {
        obj[key] = null;
      }
    } else {
      obj[key] = unquoteScalar(valPart);
    }
  }

  function parseMapping(mapIndent) {
    const obj = {};
    while (idx < lines.length) {
      const cur = lines[idx];
      if (cur.indent < mapIndent) break;
      if (cur.indent > mapIndent) {
        idx++;
        continue;
      }
      if (cur.text.startsWith('- ') || cur.text === '-') break;
      const kv = splitKeyValue(cur.text);
      if (!kv) {
        idx++;
        continue;
      }
      idx++;
      assignKeyValue(obj, kv[0], kv[1], cur.indent);
    }
    return obj;
  }

  return lines.length ? parseNode(lines[0].indent) : null;
}

export function parseYamlSafe(text) {
  try {
    return parseYaml(text) || null;
  } catch {
    return null;
  }
}

function sortByOrderThenName(items, getOrder, getName) {
  return items.sort((a, b) => {
    const oa = getOrder(a);
    const ob = getOrder(b);
    if (oa !== ob) return oa - ob;
    return String(getName(a)).localeCompare(String(getName(b)));
  });
}

export function buildProjectTree(slug, fileMap) {
  const projectDoc = fileMap.get('testrix.project.yaml');
  if (!projectDoc) return null;

  const environments = [];
  const folderNodes = new Map();

  function ensureFolder(relFolderPath) {
    if (folderNodes.has(relFolderPath)) return folderNodes.get(relFolderPath);
    const manifest = fileMap.get(`${relFolderPath}/_folder.yaml`);
    const baseName = relFolderPath.split('/').pop();
    const node = {
      relPath: relFolderPath,
      doc: manifest || { id: baseName, name: baseName, order: 0, description: '' },
      folders: [],
      requests: [],
    };
    folderNodes.set(relFolderPath, node);
    return node;
  }

  const rootRequests = [];

  for (const [relPath, doc] of fileMap.entries()) {
    if (!doc) continue;
    if (relPath.startsWith('environments/') && relPath.endsWith('.env.yaml')) {
      environments.push({ fileName: relPath.slice('environments/'.length), ...doc });
      continue;
    }
    if (relPath.endsWith('/_folder.yaml')) {
      const folderPath = relPath.slice(0, -'/_folder.yaml'.length);
      ensureFolder(folderPath);
      continue;
    }
    if (relPath.endsWith('.request.yaml')) {
      const slashIdx = relPath.lastIndexOf('/');
      if (slashIdx === -1) {
        rootRequests.push({ relPath, doc });
      } else {
        const folderPath = relPath.slice(0, slashIdx);
        const folder = ensureFolder(folderPath);
        folder.requests.push({ relPath, doc });
      }
    }
  }

  const rootFolders = [];
  for (const [folderPath, node] of folderNodes.entries()) {
    const slashIdx = folderPath.lastIndexOf('/');
    if (slashIdx === -1) {
      rootFolders.push(node);
    } else {
      const parentPath = folderPath.slice(0, slashIdx);
      const parent = ensureFolder(parentPath);
      parent.folders.push(node);
    }
  }

  function sortTree(folder) {
    sortByOrderThenName(folder.folders, (f) => f.doc?.order ?? 0, (f) => f.doc?.name ?? '');
    sortByOrderThenName(folder.requests, (r) => r.doc?.order ?? 0, (r) => r.doc?.name ?? '');
    for (const sub of folder.folders) sortTree(sub);
  }

  sortByOrderThenName(rootFolders, (f) => f.doc?.order ?? 0, (f) => f.doc?.name ?? '');
  sortByOrderThenName(rootRequests, (r) => r.doc?.order ?? 0, (r) => r.doc?.name ?? '');
  for (const f of rootFolders) sortTree(f);
  environments.sort((a, b) => String(a.name || '').localeCompare(String(b.name || '')));

  return {
    slug,
    doc: projectDoc,
    environments,
    folders: rootFolders,
    requests: rootRequests,
  };
}
