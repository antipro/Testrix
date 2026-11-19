---
name: testrix-yaml
description: Safely create, edit, and validate Testrix YAML workspace files (`testrix.io/v1`) and verify changes with `testrix --check`.
---

# Editing Testrix YAML Files

Testrix stores projects, folders, HTTP requests, and environments as `testrix.io/v1` YAML documents on disk. Whenever you create or modify Testrix YAML files, follow this schema and **always validate your changes with `--check`**.

## 1. Preferred Workflow: Validated CLI CRUD API

Whenever possible, use the `testrix` CLI CRUD API instead of editing YAML files directly. Every command performs schema & legal checks and returns structured JSON (`{ "ok": true, "data": ... }` or `{ "ok": false, "error": ... }`):

```bash
testrix [--workspace <DIR>] project  <list|get|create|update|delete> [--project <id-or-name>] [--name <name>] [--base-url <url>] [--description <desc>] [--auth-json <json>]
testrix [--workspace <DIR>] folder   <list|create|update|delete>     --project <id-or-name> [--folder <id-or-name>] [--parent <id-or-name>] [--name <name>] [--order <n>]
testrix [--workspace <DIR>] item     <list|get|create|update|delete> --project <id-or-name> [--item <id-or-name>] [--folder <id-or-name>] [--name <name>] [--method <METHOD>] [--url <url>] [--script <curl|wget|ps>] [--spec-json <json>]
testrix [--workspace <DIR>] profile  <list|get|create|update|delete|merge> --project <id-or-name> [--profile <id-or-name>] [--from <source>] [--overwrite]
testrix [--workspace <DIR>] var      <list|get|create|update|set|delete>  --project <id-or-name> --profile <id-or-name> --key <key> [--new-key <key>] [--value <val>] [--secret <true|false>] [--enabled <true|false>]
```

If you do edit YAML files manually, follow the workflow below:

1. **Read the target file** (and nearby `testrix.project.yaml`, `_folder.yaml`, or `environments/*.env.yaml` if inheritance or variables matter).
2. **Edit or create the YAML file** following the rules below. Preserve existing `id` values on existing files so open tabs, saved responses, and references do not break.
3. **Validate with `--check`** in the console before finishing:

```bash
# Check one or more specific YAML files
cargo run -p testrix-app -- --check path/to/file.request.yaml path/to/dev.env.yaml

# Check an entire project directory
cargo run -p testrix-app -- --check path/to/project-dir

# Check an entire workspace directory
cargo run -p testrix-app -- --check path/to/workspace-dir

# Check the default/configured workspace (or pass --workspace <dir>)
cargo run -p testrix-app -- --check
```

If a prebuilt `testrix` binary is on `PATH` or in `target/`, you can also run `testrix --check <path>`.
- Exit code `0` (`OK ...` on stdout) means all checked YAML files parsed and passed schema validation.
- Non-zero exit code (`ERR ...` on stderr) means one or more files failed; fix every reported error and re-run `--check`.

---

## 2. File Naming & Document Kinds

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

## 3. Common Envelope (Required on Every File)

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

## 4. Document Schemas

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

## 5. Common YAML Pitfalls to Avoid

1. **Always quote `{{variable}}` templates when they start a value.**
   Unquoted `url: {{base_url}}/users` is invalid YAML (parsed as a flow mapping `{...}`). Always write `url: "{{base_url}}/users"`.
2. **Use uppercase HTTP methods.** Write `method: POST`, not `method: post`.
3. **Never set `timeoutMs: 0`.** `settings.timeoutMs` must be `>= 1`.
4. **Match `kind` to the filename.** Use `HttpRequest` (not `Request`) in `*.request.yaml`, `Environment` in `*.env.yaml`, `Folder` in `_folder.yaml`, `Project` in `testrix.project.yaml`.
5. **Preserve `id` when editing existing files.** Only generate a new `id` when creating a brand-new document.
6. **Always run `--check` after every edit.** Never assume hand-edited YAML is valid without running `testrix --check`.

---

## 6. Further Reading

- Full schema & auth inheritance specification: [yaml-spec.md](./yaml-spec.md)
- Post-response JavaScript runtime specification: [post-script.md](./post-script.md)
