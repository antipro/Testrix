const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

test('node-web pure-JS git sync API (independent from sharing)', async () => {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'testrix-node-sync-'));
  process.env.YAML_DIR = tmpDir;
  process.env.DB_PATH = path.join(tmpDir, 'testrix.db');
  process.env.DB_TYPE = 'sqlite';
  process.env.REQUIRE_EMAIL_VERIFICATION = 'false';
  process.env.REQUIRE_EULA = 'false';
  process.env.TESTRIX_NO_AUTOSTART = '1';

  const { initDb } = require('./src/db');
  const { createApp } = require('./server');

  const db = await initDb({ type: 'sqlite', dbPath: process.env.DB_PATH });
  const app = createApp(db);

  const server = await new Promise((resolve) => {
    const s = app.listen(0, '127.0.0.1', () => resolve(s));
  });
  const { port } = server.address();
  const baseUrl = `http://127.0.0.1:${port}`;

  async function req(p, opts = {}) {
    const res = await fetch(`${baseUrl}${p}`, {
      ...opts,
      headers: {
        'Content-Type': 'application/json',
        ...(opts.headers || {}),
      },
    });
    const body = await res.json();
    return { status: res.status, body };
  }

  try {
    // Register & get auth token
    const reg = await req('/api/auth/register', {
      method: 'POST',
      body: JSON.stringify({
        email: 'sync@example.com',
        password: 'password123',
        confirmPassword: 'password123',
      }),
    });
    assert.equal(reg.status, 201);
    const token = reg.body.token;
    assert.ok(token);
    const auth = { Authorization: `Bearer ${token}` };

    // 1. Fetch/create remote sync repo state
    const st1 = await req('/api/sync/repos/my-workspace/state?create=1', { headers: auth });
    assert.equal(st1.status, 200);
    assert.equal(st1.body.headOid, null);
    assert.deepEqual(st1.body.files, {});

    // 2. Push initial tree
    const push1 = await req('/api/sync/repos/my-workspace/push', {
      method: 'POST',
      headers: auth,
      body: JSON.stringify({
        expectedHeadOid: null,
        replaceTree: true,
        message: 'Initial push',
        files: {
          'proj/get-users.request.yaml': 'name: Get Users\nurl: /users\n',
        },
      }),
    });
    assert.equal(push1.status, 200);
    assert.equal(push1.body.committed, true);
    assert.ok(push1.body.headOid);
    const oid1 = push1.body.headOid;

    // 3. Edit a file directly in node-web
    const edit1 = await req('/api/sync/repos/my-workspace/file', {
      method: 'POST',
      headers: auth,
      body: JSON.stringify({
        filepath: 'proj/get-users.request.yaml',
        content: 'name: Get Users\nurl: /v2/users\n',
        message: 'Update URL in web',
      }),
    });
    assert.equal(edit1.status, 200);
    assert.notEqual(edit1.body.headOid, oid1);

    // 4. Stale expectedHeadOid push must return 409 Conflict
    const stalePush = await req('/api/sync/repos/my-workspace/push', {
      method: 'POST',
      headers: auth,
      body: JSON.stringify({
        expectedHeadOid: oid1,
        replaceTree: true,
        message: 'Stale push',
        files: { 'proj/get-users.request.yaml': 'name: Stale\n' },
      }),
    });
    assert.equal(stalePush.status, 409);

    // 5. Check git log and restore to oid1
    const logRes = await req(
      '/api/sync/repos/my-workspace/log?filepath=proj%2Fget-users.request.yaml',
      { headers: auth }
    );
    assert.equal(logRes.status, 200);
    assert.equal(logRes.body.commits.length, 2);

    const restoreRes = await req('/api/sync/repos/my-workspace/restore', {
      method: 'POST',
      headers: auth,
      body: JSON.stringify({
        oid: oid1,
        filepath: 'proj/get-users.request.yaml',
      }),
    });
    assert.equal(restoreRes.status, 200);
    assert.equal(restoreRes.body.content, 'name: Get Users\nurl: /users\n');

    // 6. Verify Sharing (/api/my/projects) remains completely independent & empty
    const sharedRes = await req('/api/my/projects', { headers: auth });
    assert.equal(sharedRes.status, 200);
    assert.equal(sharedRes.body.projects.length, 0);
  } finally {
    server.close();
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});
