const fs = require('node:fs');
const path = require('node:path');
const git = require('isomorphic-git');

function normalizeRelPath(raw) {
  const rel = String(raw || '')
    .replace(/\\/g, '/')
    .replace(/^\/+/, '')
    .trim();
  if (!rel || rel === '.' || rel.includes('..') || rel.startsWith('.git/') || rel === '.git') {
    return null;
  }
  return rel;
}

function isEphemeralRelPath(rel) {
  return (
    rel === '.testrix-state.yaml' ||
    rel === '.testrix-history.yaml' ||
    rel.startsWith('.testrix-responses/') ||
    rel === '.DS_Store' ||
    rel.endsWith('/.DS_Store')
  );
}

async function ensureRepo(repoDir) {
  fs.mkdirSync(repoDir, { recursive: true });
  const gitDir = path.join(repoDir, '.git');
  if (!fs.existsSync(gitDir)) {
    await git.init({ fs, dir: repoDir, defaultBranch: 'main' });
  }
}

async function getHeadOid(repoDir) {
  try {
    return await git.resolveRef({ fs, dir: repoDir, ref: 'HEAD' });
  } catch {
    return null;
  }
}

async function readRepoTree(repoDir) {
  await ensureRepo(repoDir);
  const headOid = await getHeadOid(repoDir);
  if (!headOid) {
    return { headOid: null, files: {} };
  }

  const filepaths = await git.listFiles({ fs, dir: repoDir, ref: 'HEAD' });
  const files = {};
  for (const filepath of filepaths) {
    if (isEphemeralRelPath(filepath)) continue;
    try {
      const { blob } = await git.readBlob({
        fs,
        dir: repoDir,
        oid: headOid,
        filepath,
      });
      files[filepath] = Buffer.from(blob).toString('utf8');
    } catch {
      // Skip unreadable/non-blob entries
    }
  }
  return { headOid, files };
}

async function commitRepoChanges(
  repoDir,
  {
    files = {},
    deletedPaths = [],
    replaceTree = false,
    expectedHeadOid,
    authorName = 'Testrix Web',
    authorEmail = 'testrix-web@local',
    message = 'Update workspace',
  }
) {
  await ensureRepo(repoDir);
  const currentHead = await getHeadOid(repoDir);
  if (expectedHeadOid !== undefined && (expectedHeadOid || null) !== (currentHead || null)) {
    const err = new Error('Remote repository has newer commits; please pull and merge again.');
    err.statusCode = 409;
    err.currentHeadOid = currentHead;
    throw err;
  }

  const existingTracked = currentHead
    ? await git.listFiles({ fs, dir: repoDir, ref: 'HEAD' })
    : [];

  const nextRelSet = new Set();
  for (const [rawPath, content] of Object.entries(files || {})) {
    const rel = normalizeRelPath(rawPath);
    if (!rel || isEphemeralRelPath(rel)) continue;
    nextRelSet.add(rel);
    const abs = path.resolve(repoDir, rel);
    if (!abs.startsWith(path.resolve(repoDir) + path.sep)) continue;
    fs.mkdirSync(path.dirname(abs), { recursive: true });
    const text = String(content ?? '');
    const needsWrite = !fs.existsSync(abs) || fs.readFileSync(abs, 'utf8') !== text;
    if (needsWrite) {
      fs.writeFileSync(abs, text, 'utf8');
    }
    await git.add({ fs, dir: repoDir, filepath: rel });
  }

  const toDelete = new Set();
  for (const rawDel of deletedPaths || []) {
    const rel = normalizeRelPath(rawDel);
    if (rel && !isEphemeralRelPath(rel)) {
      toDelete.add(rel);
    }
  }
  if (replaceTree) {
    for (const oldPath of existingTracked) {
      if (!isEphemeralRelPath(oldPath) && !nextRelSet.has(oldPath)) {
        toDelete.add(oldPath);
      }
    }
  }

  for (const rel of toDelete) {
    const abs = path.resolve(repoDir, rel);
    if (abs.startsWith(path.resolve(repoDir) + path.sep) && fs.existsSync(abs)) {
      fs.rmSync(abs, { force: true });
    }
    try {
      await git.remove({ fs, dir: repoDir, filepath: rel });
    } catch {
      // File may not be in index
    }
  }

  const matrix = await git.statusMatrix({ fs, dir: repoDir });
  const hasChanges = matrix.some(([, head, workdir, stage]) => !(head === 1 && workdir === 1 && stage === 1));
  if (!hasChanges && currentHead) {
    return { headOid: currentHead, committed: false };
  }
  if (matrix.length === 0 && !currentHead) {
    return { headOid: null, committed: false };
  }

  const headOid = await git.commit({
    fs,
    dir: repoDir,
    author: {
      name: authorName || 'Testrix Web',
      email: authorEmail || 'testrix-web@local',
    },
    message: message || 'Sync workspace',
  });

  return { headOid, committed: true };
}

async function getRepoLog(repoDir, { filepath, depth = 50 } = {}) {
  await ensureRepo(repoDir);
  const headOid = await getHeadOid(repoDir);
  if (!headOid) return [];

  const rel = filepath ? normalizeRelPath(filepath) : undefined;
  const commits = await git.log({
    fs,
    dir: repoDir,
    depth,
    filepath: rel || undefined,
  });

  return commits.map((c) => ({
    oid: c.oid,
    shortOid: c.oid.slice(0, 7),
    message: (c.commit.message || '').trim(),
    author: c.commit.author?.name || 'Testrix',
    email: c.commit.author?.email || '',
    timestamp: c.commit.author?.timestamp || 0,
  }));
}

async function readFileAtCommit(repoDir, oid, rawFilepath) {
  const rel = normalizeRelPath(rawFilepath);
  if (!rel) {
    throw new Error('Invalid filepath');
  }
  const { blob } = await git.readBlob({
    fs,
    dir: repoDir,
    oid,
    filepath: rel,
  });
  return Buffer.from(blob).toString('utf8');
}

async function restoreFileFromCommit(repoDir, { oid, filepath, authorName, authorEmail }) {
  const rel = normalizeRelPath(filepath);
  if (!rel) {
    throw new Error('Invalid filepath');
  }
  const content = await readFileAtCommit(repoDir, oid, rel);
  const result = await commitRepoChanges(repoDir, {
    files: { [rel]: content },
    authorName,
    authorEmail,
    message: `Restore ${path.posix.basename(rel)} to ${String(oid).slice(0, 7)}`,
  });
  return { ...result, filepath: rel, content };
}

module.exports = {
  normalizeRelPath,
  ensureRepo,
  getHeadOid,
  readRepoTree,
  commitRepoChanges,
  getRepoLog,
  readFileAtCommit,
  restoreFileFromCommit,
};
