const fs = require('node:fs');
const path = require('node:path');
const yaml = require('js-yaml');

const PROJECT_MANIFEST = 'testrix.project.yaml';
const FOLDER_MANIFEST = '_folder.yaml';
const REQUEST_EXT = '.request.yaml';
const ENV_EXT = '.env.yaml';

function readYamlSafe(filePath) {
  try {
    const raw = fs.readFileSync(filePath, 'utf8');
    return yaml.load(raw) || null;
  } catch {
    return null;
  }
}

function isHidden(name) {
  return name.startsWith('.');
}

function sortByOrderThenName(items, getOrder, getName) {
  return items.sort((a, b) => {
    const oa = getOrder(a);
    const ob = getOrder(b);
    if (oa !== ob) return oa - ob;
    return String(getName(a)).localeCompare(String(getName(b)));
  });
}

function loadEnvironments(envDir) {
  if (!fs.existsSync(envDir) || !fs.statSync(envDir).isDirectory()) {
    return [];
  }
  const envs = [];
  for (const entry of fs.readdirSync(envDir, { withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith(ENV_EXT) || isHidden(entry.name)) continue;
    const doc = readYamlSafe(path.join(envDir, entry.name));
    if (doc) {
      envs.push({
        fileName: entry.name,
        ...doc,
      });
    }
  }
  return envs.sort((a, b) => String(a.name || '').localeCompare(String(b.name || '')));
}

function loadFolderRecursive(projectRoot, dirPath, hiddenRelPaths) {
  const manifestPath = path.join(dirPath, FOLDER_MANIFEST);
  const doc = fs.existsSync(manifestPath)
    ? readYamlSafe(manifestPath)
    : { id: path.basename(dirPath), name: path.basename(dirPath), order: 0, description: '' };

  const folders = [];
  const requests = [];

  for (const entry of fs.readdirSync(dirPath, { withFileTypes: true })) {
    if (isHidden(entry.name)) continue;
    const fullPath = path.join(dirPath, entry.name);
    if (entry.isDirectory()) {
      if (dirPath === projectRoot && entry.name === 'environments') continue;
      folders.push(loadFolderRecursive(projectRoot, fullPath, hiddenRelPaths));
    } else if (entry.isFile() && entry.name.endsWith(REQUEST_EXT)) {
      const relPath = path.relative(projectRoot, fullPath).replace(/\\/g, '/');
      if (hiddenRelPaths && hiddenRelPaths.has(relPath)) continue;
      const reqDoc = readYamlSafe(fullPath);
      if (reqDoc) {
        requests.push({
          relPath,
          doc: reqDoc,
        });
      }
    }
  }

  sortByOrderThenName(folders, (f) => f.doc?.order ?? 0, (f) => f.doc?.name ?? '');
  sortByOrderThenName(requests, (r) => r.doc?.order ?? 0, (r) => r.doc?.name ?? '');

  return {
    relPath: path.relative(projectRoot, dirPath).replace(/\\/g, '/'),
    doc: doc || {},
    folders,
    requests,
  };
}

function loadProjectFromDir(projectDir, options = {}) {
  const manifestPath = path.join(projectDir, PROJECT_MANIFEST);
  if (!fs.existsSync(manifestPath)) return null;
  const projectDoc = readYamlSafe(manifestPath);
  if (!projectDoc) return null;

  const hiddenRelPaths = options.hiddenRelPaths || null;
  const environments = loadEnvironments(path.join(projectDir, 'environments'));
  const rootFolders = [];
  const rootRequests = [];

  for (const entry of fs.readdirSync(projectDir, { withFileTypes: true })) {
    if (isHidden(entry.name)) continue;
    const fullPath = path.join(projectDir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === 'environments') continue;
      rootFolders.push(loadFolderRecursive(projectDir, fullPath, hiddenRelPaths));
    } else if (entry.isFile() && entry.name.endsWith(REQUEST_EXT)) {
      const relPath = path.relative(projectDir, fullPath).replace(/\\/g, '/');
      if (hiddenRelPaths && hiddenRelPaths.has(relPath)) continue;
      const reqDoc = readYamlSafe(fullPath);
      if (reqDoc) {
        rootRequests.push({
          relPath,
          doc: reqDoc,
        });
      }
    }
  }

  sortByOrderThenName(rootFolders, (f) => f.doc?.order ?? 0, (f) => f.doc?.name ?? '');
  sortByOrderThenName(rootRequests, (r) => r.doc?.order ?? 0, (r) => r.doc?.name ?? '');

  return {
    slug: options.slug || path.basename(projectDir),
    dirPath: projectDir,
    doc: projectDoc,
    environments,
    folders: rootFolders,
    requests: rootRequests,
  };
}

function flattenProjectItems(projectTree) {
  const items = [];
  function walkFolder(folder) {
    for (const r of folder.requests || []) {
      items.push({
        relPath: r.relPath,
        itemId: r.doc?.id || r.relPath,
        name: r.doc?.name || r.relPath,
        method: (r.doc?.request?.method || 'GET').toUpperCase(),
        url: r.doc?.request?.url || '',
      });
    }
    for (const sub of folder.folders || []) {
      walkFolder(sub);
    }
  }
  for (const r of projectTree.requests || []) {
    items.push({
      relPath: r.relPath,
      itemId: r.doc?.id || r.relPath,
      name: r.doc?.name || r.relPath,
      method: (r.doc?.request?.method || 'GET').toUpperCase(),
      url: r.doc?.request?.url || '',
    });
  }
  for (const f of projectTree.folders || []) {
    walkFolder(f);
  }
  return items;
}

/**
 * Discovers root-level projects placed directly in YAML_DIR (excluding `users/`).
 */
function discoverRootProjects(yamlDir) {
  if (!fs.existsSync(yamlDir)) return [];
  const projects = [];
  for (const entry of fs.readdirSync(yamlDir, { withFileTypes: true })) {
    if (!entry.isDirectory() || isHidden(entry.name) || entry.name === 'users') continue;
    const projectDir = path.join(yamlDir, entry.name);
    const manifestPath = path.join(projectDir, PROJECT_MANIFEST);
    if (!fs.existsSync(manifestPath)) continue;
    const doc = readYamlSafe(manifestPath);
    if (doc) {
      projects.push({
        slug: entry.name,
        name: doc.name || entry.name,
        description: doc.description || '',
        dirPath: projectDir,
        source: 'env_dir',
      });
    }
  }
  return projects.sort((a, b) => a.name.localeCompare(b.name));
}

module.exports = {
  loadProjectFromDir,
  flattenProjectItems,
  discoverRootProjects,
};
