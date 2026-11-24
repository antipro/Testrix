---
name: testrix-yaml
description: Manage Testrix projects, folders, HTTP requests, environments, and variables using the installed `testrix` CLI (preferred), or validate/edit `testrix.io/v1` YAML workspace files.
---

# Working with Testrix Workspaces

Testrix stores projects, folders, HTTP requests, and environments as `testrix.io/v1` YAML documents in the user's configured workspace (`~/.testrix/config.yaml`).

## 1. Strict Rules: Use the Installed `testrix` CLI First

1. **Always use the installed `testrix` CLI** — do **not** edit `.yaml` files directly unless the CLI does not support a specific field you need.
2. **Never search the filesystem for `testrix.project.yaml` or `*.yaml` files**, and **never look for or run `cargo run` in a Testrix source code repository**.
   - Run `testrix` from `PATH`.
   - If `testrix` is not on `PATH`, use the standard installed binary path:
     - **Windows**: `C:\Program Files\Testrix\testrix.exe`
     - **macOS**: `/Applications/Testrix.app/Contents/MacOS/testrix`
     - **Linux**: `/usr/bin/testrix`
3. **Omit `--workspace` by default.** `testrix` automatically resolves the user's active workspace from `~/.testrix/config.yaml`. Only pass `--workspace <DIR>` if the user explicitly gives you a workspace path (never pass a source code repo as `--workspace`).
4. **Discover first via CLI:**
   ```bash
   testrix project list
   testrix folder list --project <project-id-or-name>
   testrix item list --project <project-id-or-name>
   testrix item get --project <project-id-or-name> --item <item-id-or-name>
   ```

## 2. CLI CRUD Reference (Preferred)

Every CLI command performs schema & legal checks and returns structured JSON (`{ "ok": true, "data": ... }` or `{ "ok": false, "error": ... }`):

```bash
testrix project  <list|get|create|update|delete> [--project <id-or-name>] [--name <name>] [--base-url <url>] [--description <desc>] [--auth-json <json>]
testrix folder   <list|create|update|delete>     --project <id-or-name> [--folder <id-or-name>] [--parent <id-or-name>] [--name <name>] [--order <n>]
testrix item     <list|get|create|update|delete> --project <id-or-name> [--item <id-or-name>] [--folder <id-or-name>] [--name <name>] [--method <METHOD>] [--url <url>] [--script <curl|wget|ps>] [--spec-json <json>]
testrix profile  <list|get|create|update|delete|merge> --project <id-or-name> [--profile <id-or-name>] [--from <source>] [--overwrite]
testrix var      <list|get|create|update|set|delete>  --project <id-or-name> --profile <id-or-name> --key <key> [--new-key <key>] [--value <val>] [--secret <true|false>] [--enabled <true|false>]
testrix package  <list|get|create|update|delete|add-folder|update-folder|delete-folder|add-item|remove-item> [--package <id-or-name>] [--name <name>] [--folder <id-or-name>] [--parent <id-or-name>] [--project <id-or-name>] [--item <id-or-name>]
testrix share    <list|status|config-set|login|publish|revoke> [--project <id-or-name>] [--channel <server|r2>] [--share <id>] [--access-token <token>]
testrix notify   reload
```

- Run `testrix <resource> --help` (or `testrix --help`) to see all supported resources and actions.
- Mutating CLI CRUD commands (`create`, `update`, `delete`, `set`, `merge`, `add-folder`, `update-folder`, `delete-folder`, `add-item`, `remove-item`) automatically notify any running Testrix GUI instance via IPC (Unix Domain Socket on macOS/Linux, Named Pipe on Windows) to reload the workspace.
- Package examples:
  ```bash
  testrix package list
  testrix package create --name "Smoke Suite"
  testrix package add-folder --package "Smoke Suite" --name "Auth"
  testrix package add-item --package "Smoke Suite" --folder "Auth" --project <project-id-or-name> --item <item-id-or-name>
  testrix package remove-item --package "Smoke Suite" --folder "Auth" --item <item-id-or-name>
  testrix package delete --package "Smoke Suite"
  ```
- Sharing examples:
  ```bash
  testrix share status
  testrix share list
  testrix share publish --project <project-id-or-name> [--channel server|r2]
  testrix share revoke --project <project-id-or-name> [--channel server|r2]
  ```

## 3. Fallback: Manual YAML Editing & `--check`

Only edit YAML files directly if a specific operation is not covered by the CLI CRUD commands above. If you must edit YAML manually:

1. **Locate the workspace** by reading `~/.testrix/config.yaml` (`workspaceDir`) or inspecting `testrix project list` output — never scan the disk.
2. **Read the target file** (and nearby `testrix.project.yaml`, `_folder.yaml`, or `environments/*.env.yaml` if inheritance or variables matter).
3. **Edit or create the YAML file** following the schema below. Preserve existing `id` values on existing files so open tabs, saved responses, and references do not break.
4. **Validate with `testrix --check` and notify the running GUI to reload** before finishing:

```bash
# Check the configured workspace
testrix --check

# Notify the running Testrix GUI app to reload YAML changes immediately (via UDS on Unix / Named Pipe on Windows)
testrix notify reload
```

# Check one or more specific YAML files or a project directory
testrix --check path/to/file.request.yaml path/to/dev.env.yaml
testrix --check path/to/project-dir
```

- Exit code `0` (`OK ...` on stdout) means all checked YAML files parsed and passed schema validation.
- Non-zero exit code (`ERR ...` on stderr) means one or more files failed; fix every reported error and re-run `testrix --check`.

---

## 4. File Naming & Document Kinds

The filename **must** match `kind`:

| Filename Pattern | Location | Required `kind` |
|---|---|---|
| `testrix.project.yaml` | `<workspace>/<project-slug>/` | `Project` |
| `_folder.yaml` | `<project-root>/<folder>/` (optional) | `Folder` |
| `*.request.yaml` | `<project-root>/` or any folder | `HttpRequest` |
| `*.env.yaml` | `<project-root>/environments/` | `Environment` |

- Directories/files starting with `.` (such as `.testrix-state.yaml`, `.testrix-responses/`) are internal/hidden.
- Slugs should use lowercase ASCII `[a-z0-9-]`.

---

## 5. Common Envelope (Required on Every File)

Every Testrix YAML document must include:

```yaml
apiVersion: testrix.io/v1
kind: HttpRequest          # Project | Folder | HttpRequest | Environment
id: req-users-create       # Non-empty, unique within the project/workspace
name: Create User          # Non-empty display name
description: Optional note # Optional
```

Validation rules:
- `apiVersion` must be exactly `"testrix.io/v1"`.
- `kind` must match the filename role.
- `id` and `name` must be non-empty after trimming whitespace.

---

## 6. Document Schemas

### 4.1 `Project` (`testrix.project.yaml`)

```yaml
apiVersion: testrix.io/v1
kind: Project
id: proj-starter
name: Starter Platform API
description: Core REST endpoints
baseUrl: "{{base_url}}"
auth:
  type: bearer
  token: "{{api_token}}"
  prefix: Bearer
defaultHeaders:
  - name: Accept
    value: application/json
    enabled: true
    description: Prefer JSON responses
```

### 4.2 `Folder` (`_folder.yaml`)

```yaml
apiVersion: testrix.io/v1
kind: Folder
id: fld-users
name: Users
description: User management endpoints
order: 10
auth:
  type: inherit
```

- `order`: `i32` (default `0`, ascending sort weight).
- `auth`: defaults to `{ type: inherit }`.

### 4.3 `HttpRequest` (`*.request.yaml`)

```yaml
apiVersion: testrix.io/v1
kind: HttpRequest
id: req-users-create
name: Create User
description: Provision a new account
order: 10
request:
  method: POST
  url: "{{base_url}}/users"
  params:
    - name: source
      value: cli
      enabled: true
  headers:
    - name: X-Trace-Id
      value: "{{trace_id}}"
      enabled: true
  auth:
    type: inherit
  body:
    type: json
    content: |
      {
        "username": "{{sample_user}}"
      }
  postScript: |
    if (response.status === 201) {
      const data = response.json();
      environment.var["user_id"] = String(data.id);
    }
  settings:
    timeoutMs: 30000
    followRedirects: true
    verifyTls: true
```

Key field rules:
- `request.method`: **uppercase** `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, or `OPTIONS` (default `GET`).
- `request.url`: full URL or `/path` relative to `Project.baseUrl`.
- `request.params` / `request.headers`: list of `{ name, value, enabled, description }` (`enabled` defaults to `true`).
- `request.settings.timeoutMs`: must be `>= 1` (default `30000`).
- `request.postScript`: optional QuickJS script with `environment`, `request`, and `response` globals (see [post-script.md](./post-script.md)).
- `request.responseExamples`: optional list of saved response snapshots (`{ id, name, statusCode, reason, contentType, headers, body }`).
- `request.responseMeta`: optional extracted JSON/XML schema tree (`{ name, type, example, description, children }`) with editable field descriptions.

#### `request.body` Variants (Tagged by `type`)

1. `none`:
   ```yaml
   body:
     type: none
   ```
2. `json`:
   ```yaml
   body:
     type: json
     content: '{"hello": "world"}'
   ```
3. `raw` (`rawFormat`: `text` | `xml` | `html` | `javascript` | `yaml`):
   ```yaml
   body:
     type: raw
     rawFormat: xml
     content: "<ping/>"
   ```
4. `formUrlEncoded`:
   ```yaml
   body:
     type: formUrlEncoded
     fields:
       - name: grant_type
         value: client_credentials
         enabled: true
   ```
5. `multipart` (note: `MultipartField` uses snake_case keys `field_type`, `file_name`, `content_type`):
   ```yaml
   body:
     type: multipart
     fields:
       - name: metadata
         field_type: text
         value: '{"env":"{{env_name}}"}'
         enabled: true
       - name: artifact
         field_type: file
         value: ./payload.bin
         file_name: payload.bin
         content_type: application/octet-stream
         enabled: true
   ```
6. `binary`:
   ```yaml
   body:
     type: binary
     filePath: ./archive.tar.gz
     contentType: application/gzip
   ```

### 4.4 `Environment` (`environments/*.env.yaml`)

```yaml
apiVersion: testrix.io/v1
kind: Environment
id: env-development
name: Development
variables:
  - key: base_url
    value: https://httpbin.org
    enabled: true
    secret: false
    description: Sandbox base URL
  - key: api_token
    value: dev-token
    enabled: true
    secret: true
```

- Note: `Environment.variables` uses `key` + `value`, whereas `params`, `headers`, `defaultHeaders`, and `formUrlEncoded.fields` use `name` + `value`.

### 4.5 `AuthConfig` Variants (Tagged by `type`)

- `inherit` (valid on `Folder`, `HttpRequest`; resolved as `none` on `Project`)
- `none`
- `bearer`: `token`, `prefix` (optional, default `"Bearer"`)
- `basic`: `username`, `password`
- `apiKey`: `key`, `value`, `in` (`header` or `query`)

---

## 7. Common YAML Pitfalls to Avoid

1. **Always quote `{{variable}}` templates when they start a value.**
   Unquoted `url: {{base_url}}/users` is invalid YAML (parsed as a flow mapping `{...}`). Always write `url: "{{base_url}}/users"`.
2. **Use uppercase HTTP methods.** Write `method: POST`, not `method: post`.
3. **Never set `timeoutMs: 0`.** `settings.timeoutMs` must be `>= 1`.
4. **Match `kind` to the filename.** Use `HttpRequest` (not `Request`) in `*.request.yaml`, `Environment` in `*.env.yaml`, `Folder` in `_folder.yaml`, `Project` in `testrix.project.yaml`.
5. **Preserve `id` when editing existing files.** Only generate a new `id` when creating a brand-new document.
6. **Always run `--check` after every edit.** Never assume hand-edited YAML is valid without running `testrix --check`.

---

## 8. Further Reading

- Full schema & auth inheritance specification: [yaml-spec.md](./yaml-spec.md)
- Post-response JavaScript runtime specification: [post-script.md](./post-script.md)
