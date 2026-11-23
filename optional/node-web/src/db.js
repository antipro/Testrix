const fs = require('node:fs');
const path = require('node:path');
const { DatabaseSync } = require('node:sqlite');
const mysql = require('mysql2/promise');

const SQLITE_SCHEMA = `
  PRAGMA journal_mode = WAL;
  PRAGMA foreign_keys = ON;

  CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
  );

  CREATE TABLE IF NOT EXISTS shared_projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    dir_path TEXT NOT NULL,
    is_public INTEGER NOT NULL DEFAULT 1,
    access_token TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
  );

  CREATE TABLE IF NOT EXISTS shared_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES shared_projects(id) ON DELETE CASCADE,
    rel_path TEXT NOT NULL,
    item_id TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL,
    method TEXT NOT NULL DEFAULT 'GET',
    url TEXT NOT NULL DEFAULT '',
    is_visible INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(project_id, rel_path)
  );

  CREATE TABLE IF NOT EXISTS email_verifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    email TEXT NOT NULL,
    code TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    used INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
  );

  CREATE TABLE IF NOT EXISTS git_repos (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    slug TEXT NOT NULL,
    name TEXT NOT NULL,
    dir_path TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(user_id, slug)
  );
`;

const MYSQL_STATEMENTS = [
  `CREATE TABLE IF NOT EXISTS users (
    id INT AUTO_INCREMENT PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    password_hash VARCHAR(255) NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,

  `CREATE TABLE IF NOT EXISTS shared_projects (
    id INT AUTO_INCREMENT PRIMARY KEY,
    user_id INT NOT NULL,
    slug VARCHAR(255) NOT NULL UNIQUE,
    name VARCHAR(255) NOT NULL,
    description TEXT NOT NULL,
    dir_path VARCHAR(1024) NOT NULL,
    is_public TINYINT(1) NOT NULL DEFAULT 1,
    access_token VARCHAR(255) NOT NULL DEFAULT '',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT fk_shared_projects_user
      FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,

  `CREATE TABLE IF NOT EXISTS shared_items (
    id INT AUTO_INCREMENT PRIMARY KEY,
    project_id INT NOT NULL,
    rel_path VARCHAR(500) NOT NULL,
    item_id VARCHAR(255) NOT NULL DEFAULT '',
    name VARCHAR(255) NOT NULL,
    method VARCHAR(32) NOT NULL DEFAULT 'GET',
    url VARCHAR(2048) NOT NULL DEFAULT '',
    is_visible TINYINT(1) NOT NULL DEFAULT 1,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY uniq_project_rel_path (project_id, rel_path),
    CONSTRAINT fk_shared_items_project
      FOREIGN KEY (project_id) REFERENCES shared_projects(id) ON DELETE CASCADE
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,

  `CREATE TABLE IF NOT EXISTS email_verifications (
    id INT AUTO_INCREMENT PRIMARY KEY,
    email VARCHAR(255) NOT NULL,
    code VARCHAR(32) NOT NULL,
    expires_at BIGINT NOT NULL,
    used TINYINT(1) NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_email_verifications_email (email)
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,

  `CREATE TABLE IF NOT EXISTS git_repos (
    id INT AUTO_INCREMENT PRIMARY KEY,
    user_id INT NOT NULL,
    slug VARCHAR(255) NOT NULL,
    name VARCHAR(255) NOT NULL,
    dir_path VARCHAR(1024) NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY uniq_git_repos_user_slug (user_id, slug),
    CONSTRAINT fk_git_repos_user
      FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,
];

function initSqlite(dbPath) {
  const dir = path.dirname(dbPath);
  fs.mkdirSync(dir, { recursive: true });

  const raw = new DatabaseSync(dbPath);
  raw.exec(SQLITE_SCHEMA);

  const cols = raw.prepare('PRAGMA table_info(shared_projects)').all();
  if (!cols.some((c) => c.name === 'access_token')) {
    raw.exec("ALTER TABLE shared_projects ADD COLUMN access_token TEXT NOT NULL DEFAULT ''");
  }

  return {
    type: 'sqlite',
    async get(sql, params = []) {
      return raw.prepare(sql).get(...params);
    },
    async all(sql, params = []) {
      return raw.prepare(sql).all(...params);
    },
    async run(sql, params = []) {
      const info = raw.prepare(sql).run(...params);
      return {
        changes: Number(info.changes || 0),
        lastInsertRowid: Number(info.lastInsertRowid || 0),
      };
    },
    async exec(sql) {
      raw.exec(sql);
    },
    async close() {
      raw.close();
    },
  };
}

async function initMysql(options) {
  const poolConfig = options.mysqlUrl
    ? options.mysqlUrl
    : {
        host: options.host || '127.0.0.1',
        port: Number(options.port || 3306),
        user: options.user || 'root',
        password: options.password || '',
        database: options.database || 'testrix',
        waitForConnections: true,
        connectionLimit: Number(options.connectionLimit || 10),
        charset: 'utf8mb4',
      };

  const pool = mysql.createPool(poolConfig);

  try {
    for (const stmt of MYSQL_STATEMENTS) {
      await pool.query(stmt);
    }

    const [colRows] = await pool.query(
      `SELECT COUNT(*) AS cnt
       FROM information_schema.COLUMNS
       WHERE TABLE_SCHEMA = DATABASE()
         AND TABLE_NAME = 'shared_projects'
         AND COLUMN_NAME = 'access_token'`
    );
    if (Number(colRows[0]?.cnt || 0) === 0) {
      await pool.query(
        "ALTER TABLE shared_projects ADD COLUMN access_token VARCHAR(255) NOT NULL DEFAULT ''"
      );
    }
  } catch (err) {
    await pool.end().catch(() => {});
    throw err;
  }

  return {
    type: 'mysql',
    async get(sql, params = []) {
      const [rows] = await pool.execute(sql, params);
      return rows[0];
    },
    async all(sql, params = []) {
      const [rows] = await pool.execute(sql, params);
      return rows;
    },
    async run(sql, params = []) {
      const [result] = await pool.execute(sql, params);
      return {
        changes: Number(result.affectedRows || 0),
        lastInsertRowid: Number(result.insertId || 0),
      };
    },
    async exec(sql) {
      await pool.query(sql);
    },
    async close() {
      await pool.end();
    },
  };
}

async function initDb(options = {}) {
  const config = typeof options === 'string' ? { type: 'sqlite', dbPath: options } : options;
  const dbType = String(config.type || process.env.DB_TYPE || 'sqlite')
    .trim()
    .toLowerCase();

  if (dbType === 'sqlite') {
    if (!config.dbPath) {
      throw new Error('DB_PATH is required when DB_TYPE=sqlite');
    }
    return initSqlite(config.dbPath);
  }

  if (dbType === 'mysql') {
    return initMysql(config);
  }

  throw new Error(`Unsupported DB_TYPE "${dbType}". Supported values: "sqlite", "mysql".`);
}

module.exports = { initDb };
