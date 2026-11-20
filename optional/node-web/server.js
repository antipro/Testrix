const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const express = require('express');
const { initDb } = require('./src/db');
const { sendVerificationCode } = require('./src/mailer');
const {
  COOKIE_NAME,
  hashPassword,
  verifyPassword,
  signToken,
  parseCookies,
  attachUser,
  requireAuth,
} = require('./src/auth');
const {
  loadProjectFromDir,
  flattenProjectItems,
  discoverRootProjects,
} = require('./src/yamlReader');

const PORT = Number(process.env.PORT || 3456);
const YAML_DIR = path.resolve(process.env.YAML_DIR || process.env.DATA_DIR || path.join(__dirname, 'data'));
const DB_TYPE = String(process.env.DB_TYPE || 'sqlite').trim().toLowerCase();
const DB_PATH = path.resolve(process.env.DB_PATH || path.join(YAML_DIR, 'testrix.db'));
const PUBLIC_BASE_URL = (process.env.PUBLIC_BASE_URL || `http://localhost:${PORT}`).replace(/\/+$/, '');
const REQUIRE_EMAIL_VERIFICATION = !['false', '0', 'no'].includes(
  String(process.env.REQUIRE_EMAIL_VERIFICATION || 'true').trim().toLowerCase()
);

function sanitizeSlug(raw) {
  return String(raw || 'project')
    .toLowerCase()
    .replace(/[^a-z0-9-_]+/g, '-')
    .replace(/^-+|-+$/g, '') || 'project';
}

function setAuthCookie(res, token) {
  res.cookie(COOKIE_NAME, token, {
    httpOnly: true,
    sameSite: 'lax',
    maxAge: 7 * 24 * 60 * 60 * 1000,
    path: '/',
  });
}

function asyncHandler(fn) {
  return (req, res, next) => {
    Promise.resolve(fn(req, res, next)).catch(next);
  };
}

function createApp(db) {
  const app = express();
  app.use(express.json({ limit: '25mb' }));
  app.use(attachUser(db));
  app.use(express.static(path.join(__dirname, 'public')));

  // Health check
  app.get('/api/health', (_req, res) => {
    res.json({ ok: true, yamlDir: YAML_DIR, dbType: db.type });
  });

  // Auth endpoints
  app.get('/api/auth/config', (_req, res) => {
    res.json({ requireEmailVerification: REQUIRE_EMAIL_VERIFICATION });
  });

  app.post(
    '/api/auth/send-code',
    asyncHandler(async (req, res) => {
      if (!REQUIRE_EMAIL_VERIFICATION) {
        return res.json({ ok: true, skipped: true, message: 'Email verification is disabled' });
      }
      const email = String(req.body?.email || '').trim().toLowerCase();
      if (!email || !email.includes('@')) {
        return res.status(400).json({ error: 'Valid email is required' });
      }

      const existing = await db.get('SELECT id FROM users WHERE email = ?', [email]);
      if (existing) {
        return res.status(409).json({ error: 'Email already registered' });
      }

      const code = crypto.randomInt(100000, 1000000).toString();
      const expiresAt = Date.now() + 10 * 60 * 1000;

      await db.run('UPDATE email_verifications SET used = 1 WHERE email = ? AND used = 0', [email]);
      await db.run('INSERT INTO email_verifications (email, code, expires_at) VALUES (?, ?, ?)', [
        email,
        code,
        expiresAt,
      ]);

      try {
        await sendVerificationCode(email, code);
        res.json({ ok: true, message: 'Verification code sent to your email' });
      } catch (err) {
        res.status(500).json({ error: err.message || 'Failed sending verification email' });
      }
    })
  );

  app.post(
    '/api/auth/register',
    asyncHandler(async (req, res) => {
      const email = String(req.body?.email || '').trim().toLowerCase();
      const password = String(req.body?.password || '');
      const confirmPassword = String(req.body?.confirmPassword || '');
      const code = String(req.body?.code || '').trim();

      if (!email || !email.includes('@')) {
        return res.status(400).json({ error: 'Valid email is required' });
      }
      if (password.length < 6) {
        return res.status(400).json({ error: 'Password must be at least 6 characters' });
      }
      if (password !== confirmPassword) {
        return res.status(400).json({ error: 'Passwords do not match' });
      }
      if (REQUIRE_EMAIL_VERIFICATION && !code) {
        return res.status(400).json({ error: 'Verification code is required' });
      }

      const existing = await db.get('SELECT id FROM users WHERE email = ?', [email]);
      if (existing) {
        return res.status(409).json({ error: 'Email already registered' });
      }

      if (REQUIRE_EMAIL_VERIFICATION) {
        const verif = await db.get(
          `SELECT id, code, expires_at FROM email_verifications
           WHERE email = ? AND used = 0
           ORDER BY id DESC LIMIT 1`,
          [email]
        );

        if (!verif || verif.code !== code) {
          return res.status(400).json({ error: 'Invalid verification code' });
        }
        if (verif.expires_at < Date.now()) {
          return res.status(400).json({ error: 'Verification code has expired' });
        }

        await db.run('UPDATE email_verifications SET used = 1 WHERE id = ?', [verif.id]);
      }

      const passwordHash = hashPassword(password);
      const info = await db.run('INSERT INTO users (email, password_hash) VALUES (?, ?)', [
        email,
        passwordHash,
      ]);

      const user = { id: Number(info.lastInsertRowid), email };
      const token = signToken({ userId: user.id, email: user.email });
      setAuthCookie(res, token);
      res.status(201).json({ token, user });
    })
  );

  app.post(
    '/api/auth/login',
    asyncHandler(async (req, res) => {
      const email = String(req.body?.email || '').trim().toLowerCase();
      const password = String(req.body?.password || '');

      const row = await db.get('SELECT id, email, password_hash FROM users WHERE email = ?', [
        email,
      ]);
      if (!row || !verifyPassword(password, row.password_hash)) {
        return res.status(401).json({ error: 'Invalid email or password' });
      }

      const user = { id: row.id, email: row.email };
      const token = signToken({ userId: user.id, email: user.email });
      setAuthCookie(res, token);
      res.json({ token, user });
    })
  );

  app.post('/api/auth/logout', (_req, res) => {
    res.clearCookie(COOKIE_NAME, { path: '/' });
    res.json({ ok: true });
  });

  app.get('/api/auth/me', (req, res) => {
    if (!req.user) {
      return res.status(401).json({ error: 'Not authenticated' });
    }
    res.json({ user: req.user });
  });

  // Public / Viewer endpoints (reads YAML directly from YAML_DIR)
  app.get(
    '/api/public/projects',
    asyncHandler(async (req, res) => {
      const envProjects = discoverRootProjects(YAML_DIR).map((p) => ({
        slug: p.slug,
        name: p.name,
        description: p.description,
        source: 'yaml_dir',
        isPublic: true,
        ownerEmail: null,
      }));

      const dbRows = await db.all(
        `SELECT sp.id, sp.slug, sp.name, sp.description, sp.is_public, sp.updated_at, u.email AS owner_email
         FROM shared_projects sp
         JOIN users u ON u.id = sp.user_id
         WHERE sp.is_public = 1 OR sp.user_id = ?
         ORDER BY sp.updated_at DESC`,
        [req.user ? req.user.id : -1]
      );

      const seen = new Set(envProjects.map((p) => p.slug));
      const merged = [...envProjects];
      for (const row of dbRows) {
        if (seen.has(row.slug)) continue;
        seen.add(row.slug);
        merged.push({
          id: row.id,
          slug: row.slug,
          name: row.name,
          description: row.description,
          source: 'shared',
          isPublic: Boolean(row.is_public),
          ownerEmail: row.owner_email,
          updatedAt: row.updated_at,
        });
      }

      res.json({ projects: merged });
    })
  );

  app.get(
    '/api/public/projects/:slug',
    asyncHandler(async (req, res) => {
      const slug = sanitizeSlug(req.params.slug);

      // 1. Check DB shared project first
      const dbProj = await db.get(
        `SELECT sp.*, u.email AS owner_email
         FROM shared_projects sp
         JOIN users u ON u.id = sp.user_id
         WHERE sp.slug = ?`,
        [slug]
      );

      if (dbProj) {
        const isOwner = req.user && req.user.id === dbProj.user_id;
        if (!dbProj.is_public && !isOwner) {
          return res.status(403).json({ error: 'This shared project is private' });
        }
        if (dbProj.access_token && !isOwner) {
          const cookies = parseCookies(req.headers.cookie);
          const provided =
            req.headers['x-access-token'] ||
            req.query.token ||
            cookies[`testrix_access_${dbProj.slug}`] ||
            '';
          if (String(provided) !== dbProj.access_token) {
            return res.status(403).json({
              error: 'Access token required to view this project',
              accessTokenRequired: true,
            });
          }
        }
        const hiddenRows = await db.all(
          'SELECT rel_path FROM shared_items WHERE project_id = ? AND is_visible = 0',
          [dbProj.id]
        );
        const hiddenRelPaths = new Set(hiddenRows.map((r) => r.rel_path));
        const tree = loadProjectFromDir(dbProj.dir_path, { slug: dbProj.slug, hiddenRelPaths });
        if (!tree) {
          return res.status(404).json({ error: 'Project YAML files not found on disk' });
        }
        return res.json({
          project: {
            ...tree,
            meta: {
              id: dbProj.id,
              isPublic: Boolean(dbProj.is_public),
              ownerEmail: dbProj.owner_email,
              source: 'shared',
            },
          },
        });
      }

      // 2. Fall back to root YAML_DIR/<slug>
      const directDir = path.join(YAML_DIR, slug);
      if (directDir.startsWith(YAML_DIR) && fs.existsSync(directDir)) {
        const tree = loadProjectFromDir(directDir, { slug });
        if (tree) {
          return res.json({
            project: {
              ...tree,
              meta: {
                isPublic: true,
                source: 'yaml_dir',
              },
            },
          });
        }
      }

      return res.status(404).json({ error: 'Project not found' });
    })
  );

  app.post(
    '/api/public/projects/:slug/unlock',
    asyncHandler(async (req, res) => {
      const slug = sanitizeSlug(req.params.slug);
      const dbProj = await db.get('SELECT slug, access_token FROM shared_projects WHERE slug = ?', [
        slug,
      ]);
      if (!dbProj) {
        return res.status(404).json({ error: 'Project not found' });
      }
      const provided = String(req.body?.accessToken || '');
      if (dbProj.access_token && provided !== dbProj.access_token) {
        return res.status(401).json({ error: 'Invalid access token' });
      }
      res.cookie(`testrix_access_${dbProj.slug}`, provided, {
        httpOnly: true,
        sameSite: 'lax',
        maxAge: 7 * 24 * 60 * 60 * 1000,
        path: '/',
      });
      res.json({ ok: true });
    })
  );

  // Authenticated user project & item management
  app.get(
    '/api/my/projects',
    requireAuth,
    asyncHandler(async (req, res) => {
      const rows = await db.all(
        `SELECT sp.*,
          (SELECT COUNT(*) FROM shared_items si WHERE si.project_id = sp.id) AS total_items,
          (SELECT COUNT(*) FROM shared_items si WHERE si.project_id = sp.id AND si.is_visible = 1) AS visible_items
         FROM shared_projects sp
         WHERE sp.user_id = ?
         ORDER BY sp.updated_at DESC`,
        [req.user.id]
      );

      res.json({
        projects: rows.map((r) => ({
          id: r.id,
          slug: r.slug,
          name: r.name,
          description: r.description,
          isPublic: Boolean(r.is_public),
          totalItems: r.total_items,
          visibleItems: r.visible_items,
          shareUrl: `${PUBLIC_BASE_URL}/s/${r.slug}`,
          updatedAt: r.updated_at,
        })),
      });
    })
  );

  app.post(
    '/api/projects/upload',
    requireAuth,
    asyncHandler(async (req, res) => {
      const rawSlug = req.body?.projectSlug || req.body?.projectName || 'project';
      const baseSlug = sanitizeSlug(rawSlug);
      const files = Array.isArray(req.body?.files) ? req.body.files : [];

      if (files.length === 0) {
        return res.status(400).json({ error: 'No YAML files provided' });
      }

      // Scope slug if another user already owns baseSlug
      let slug = baseSlug;
      const existing = await db.get('SELECT id, user_id FROM shared_projects WHERE slug = ?', [slug]);
      if (existing && existing.user_id !== req.user.id) {
        slug = `${baseSlug}-u${req.user.id}`;
      }

      const projectDir = path.join(YAML_DIR, 'users', String(req.user.id), slug);
      fs.rmSync(projectDir, { recursive: true, force: true });
      fs.mkdirSync(projectDir, { recursive: true });

      let writtenCount = 0;
      for (const file of files) {
        const relPath = String(file?.path || '').replace(/\\/g, '/').replace(/^\/+/, '');
        if (!relPath || relPath.includes('..') || !(relPath.endsWith('.yaml') || relPath.endsWith('.yml'))) {
          continue;
        }
        const targetPath = path.resolve(projectDir, relPath);
        if (!targetPath.startsWith(projectDir + path.sep)) {
          continue;
        }
        fs.mkdirSync(path.dirname(targetPath), { recursive: true });
        fs.writeFileSync(targetPath, String(file.content || ''), 'utf8');
        writtenCount += 1;
      }

      if (writtenCount === 0) {
        return res.status(400).json({ error: 'No valid .yaml files were uploaded' });
      }

      const tree = loadProjectFromDir(projectDir, { slug });
      const projectName = String(req.body?.projectName || tree?.doc?.name || slug);
      const projectDesc = String(tree?.doc?.description || '');
      const isPublic = req.body?.isPublic === false ? 0 : 1;
      const accessToken = String(req.body?.accessToken || '').trim();

      const existingOwn = await db.get(
        'SELECT id FROM shared_projects WHERE user_id = ? AND slug = ?',
        [req.user.id, slug]
      );

      let projectId;
      if (existingOwn) {
        projectId = existingOwn.id;
        await db.run(
          `UPDATE shared_projects
           SET name = ?, description = ?, dir_path = ?, access_token = ?, updated_at = CURRENT_TIMESTAMP
           WHERE id = ?`,
          [projectName, projectDesc, projectDir, accessToken, projectId]
        );
      } else {
        const info = await db.run(
          `INSERT INTO shared_projects (user_id, slug, name, description, dir_path, is_public, access_token)
           VALUES (?, ?, ?, ?, ?, ?, ?)`,
          [req.user.id, slug, projectName, projectDesc, projectDir, isPublic, accessToken]
        );
        projectId = Number(info.lastInsertRowid);
      }

      // Sync discovered items while preserving existing visibility flags
      const prevItems = await db.all(
        'SELECT rel_path, is_visible FROM shared_items WHERE project_id = ?',
        [projectId]
      );
      const prevVis = new Map(prevItems.map((i) => [i.rel_path, i.is_visible]));

      await db.run('DELETE FROM shared_items WHERE project_id = ?', [projectId]);
      const flatItems = tree ? flattenProjectItems(tree) : [];
      for (const item of flatItems) {
        const vis = prevVis.has(item.relPath) ? prevVis.get(item.relPath) : 1;
        await db.run(
          `INSERT INTO shared_items (project_id, rel_path, item_id, name, method, url, is_visible)
           VALUES (?, ?, ?, ?, ?, ?, ?)`,
          [projectId, item.relPath, item.itemId, item.name, item.method, item.url, vis]
        );
      }

      res.json({
        ok: true,
        projectId,
        slug,
        filesWritten: writtenCount,
        itemCount: flatItems.length,
        shareUrl: `${PUBLIC_BASE_URL}/s/${slug}`,
      });
    })
  );

  app.patch(
    '/api/my/projects/:id',
    requireAuth,
    asyncHandler(async (req, res) => {
      const id = Number(req.params.id);
      const proj = await db.get('SELECT * FROM shared_projects WHERE id = ? AND user_id = ?', [
        id,
        req.user.id,
      ]);
      if (!proj) return res.status(404).json({ error: 'Project not found' });

      const isPublic = typeof req.body?.isPublic === 'boolean' ? (req.body.isPublic ? 1 : 0) : proj.is_public;
      const name = req.body?.name ? String(req.body.name).trim() : proj.name;

      await db.run(
        'UPDATE shared_projects SET is_public = ?, name = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?',
        [isPublic, name, id]
      );
      res.json({ ok: true, id, isPublic: Boolean(isPublic), name });
    })
  );

  app.delete(
    '/api/my/projects/by-slug/:slug',
    requireAuth,
    asyncHandler(async (req, res) => {
      const slug = sanitizeSlug(req.params.slug);
      const proj = await db.get('SELECT * FROM shared_projects WHERE slug = ? AND user_id = ?', [
        slug,
        req.user.id,
      ]);
      if (!proj) return res.status(404).json({ error: 'Project not found' });

      await db.run('DELETE FROM shared_projects WHERE id = ?', [proj.id]);
      if (proj.dir_path && proj.dir_path.startsWith(YAML_DIR)) {
        fs.rmSync(proj.dir_path, { recursive: true, force: true });
      }
      res.json({ ok: true });
    })
  );

  app.delete(
    '/api/my/projects/:id',
    requireAuth,
    asyncHandler(async (req, res) => {
      const id = Number(req.params.id);
      const proj = await db.get('SELECT * FROM shared_projects WHERE id = ? AND user_id = ?', [
        id,
        req.user.id,
      ]);
      if (!proj) return res.status(404).json({ error: 'Project not found' });

      await db.run('DELETE FROM shared_projects WHERE id = ?', [id]);
      if (proj.dir_path && proj.dir_path.startsWith(YAML_DIR)) {
        fs.rmSync(proj.dir_path, { recursive: true, force: true });
      }
      res.json({ ok: true });
    })
  );

  app.get(
    '/api/my/projects/:id/items',
    requireAuth,
    asyncHandler(async (req, res) => {
      const id = Number(req.params.id);
      const proj = await db.get('SELECT * FROM shared_projects WHERE id = ? AND user_id = ?', [
        id,
        req.user.id,
      ]);
      if (!proj) return res.status(404).json({ error: 'Project not found' });

      const items = await db.all(
        'SELECT * FROM shared_items WHERE project_id = ? ORDER BY rel_path ASC',
        [id]
      );
      res.json({
        project: { id: proj.id, slug: proj.slug, name: proj.name, isPublic: Boolean(proj.is_public) },
        items: items.map((i) => ({
          id: i.id,
          relPath: i.rel_path,
          itemId: i.item_id,
          name: i.name,
          method: i.method,
          url: i.url,
          isVisible: Boolean(i.is_visible),
        })),
      });
    })
  );

  app.patch(
    '/api/my/projects/:id/items/:itemId',
    requireAuth,
    asyncHandler(async (req, res) => {
      const projectId = Number(req.params.id);
      const itemId = Number(req.params.itemId);
      const proj = await db.get('SELECT id FROM shared_projects WHERE id = ? AND user_id = ?', [
        projectId,
        req.user.id,
      ]);
      if (!proj) return res.status(404).json({ error: 'Project not found' });

      const isVisible = req.body?.isVisible ? 1 : 0;
      await db.run(
        'UPDATE shared_items SET is_visible = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND project_id = ?',
        [isVisible, itemId, projectId]
      );
      res.json({ ok: true, id: itemId, isVisible: Boolean(isVisible) });
    })
  );

  // Page routes (no webpack, plain HTML)
  app.get(['/s/:slug', '/s/:slug/*', '/login', '/register', '/dashboard'], (_req, res) => {
    res.sendFile(path.join(__dirname, 'public', 'index.html'));
  });

  app.use((err, _req, res, _next) => {
    console.error(err);
    if (res.headersSent) return;
    res.status(500).json({ error: 'Internal server error' });
  });

  return app;
}

async function start() {
  fs.mkdirSync(YAML_DIR, { recursive: true });
  const db = await initDb({
    type: DB_TYPE,
    dbPath: DB_PATH,
    mysqlUrl: process.env.MYSQL_URL || process.env.DATABASE_URL,
    host: process.env.MYSQL_HOST || '127.0.0.1',
    port: Number(process.env.MYSQL_PORT || 3306),
    user: process.env.MYSQL_USER || 'root',
    password: process.env.MYSQL_PASSWORD || '',
    database: process.env.MYSQL_DATABASE || 'testrix',
    connectionLimit: Number(process.env.MYSQL_CONNECTION_LIMIT || 10),
  });

  const app = createApp(db);
  app.listen(PORT, () => {
    console.log(`Testrix Share Server listening on http://localhost:${PORT}`);
    console.log(`Database type: ${db.type}`);
    console.log(`Reading YAML projects from: ${YAML_DIR}`);
  });
}

if (require.main === module) {
  start().catch((err) => {
    console.error('Failed to start Testrix Share Server:', err);
    process.exit(1);
  });
}

module.exports = { createApp, start };
