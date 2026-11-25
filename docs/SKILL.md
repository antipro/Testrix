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

## 2. CLI CRUD, Execution & Help Reference (Preferred)

Every CLI command performs schema & legal checks and returns structured JSON (`{ "ok": true, "data": ... }` or `{ "ok": false, "error": ... }`):

```bash
testrix project  <list|get|create|update|delete> [--project <ref>] [--name <name>] [--base-url <url>] [--description <desc>] [--auth-json <json>]
testrix folder   <list|create|update|delete>     --project <ref> [--folder <ref>] [--parent <ref>] [--name <name>] [--order <n>]
testrix item     <list|get|create|update|delete|run> --project <ref> [--item <ref>] [--folder <ref>] [--id <id>] [--upsert] [--name <name>] [--description <desc>] [--order <n>] [--method <METHOD>] [--url <url>] [--script <curl|wget|ps>] [--spec-json <json|->] [--spec-file <path>] [--env <ref>] [--var <k=v>]
testrix profile  <list|get|create|update|delete|merge> [--project <ref>] [--profile <ref>] [--from <source>] [--overwrite]
testrix var      <list|get|create|update|set|delete>  [--project <ref>] --profile <ref> --key <key> [--new-key <key>] [--value <val>] [--secret <true|false>] [--enabled <true|false>]
testrix package  <list|get|create|update|delete|add-folder|update-folder|delete-folder|add-item|remove-item|run|dag> [--package <ref>] [--name <name>] [--folder <ref>] [--parent <ref>] [--project <ref>] [--item <ref>] [--env <ref>] [--var <k=v>] [--fail-fast]
testrix package  dag <get|validate|set|add-node|add-edge|remove-node> --package <ref> [...]
testrix share    <list|status|config-set|login|publish|revoke> [--project <ref>] [--channel <server|r2>] [--share <id>] [--access-token <token>]
testrix notify   reload
```

### Key CLI Rules & Pitfalls

| Topic | Behavior & Best Practice |
|---|---|
| **Running alongside GUI** | CLI CRUD and `run` commands work while the Testrix GUI is running; mutating commands automatically notify the GUI via IPC (`UDS` / `Named Pipe`) to reload. |
| **Subcommand `--help`** | Run `testrix <resource> --help` or `testrix <resource> <action> --help` (e.g. `testrix item create --help`) for exact flags, required fields, and `--spec-json` schema. |
| **Identifier resolution (`<ref>`)** | `--project`, `--folder`, `--item`, `--profile`, and `--package` accept **id**, **exact display name**, **directory/file slug** (e.g. `auth-flow`, `create-user`), or **relative path** (for folders/items). Not-found errors list available candidates. |
| **`--spec-json` / `--spec-file` merge** | `--spec-json <json\|->` (use `-` for stdin) and `--spec-file <path>` supply a full `HttpRequestSpec` object. If `--method` or `--url` is also passed, those flags **override** the spec's `method`/`url`. |
| **Stable output & Windows UTF-8** | Testrix switches the Windows console to UTF-8 (`CP_UTF8`). For scripts, pass `--out <file>` to write UTF-8 JSON directly to disk, or `--quiet` (`-q`) to print only the created/updated/deleted `id`. |
| **Manual YAML fallback** | All core fields (`--description`, `--order`, `--id`, `--upsert`, `package dag`) are supported in the CLI; only edit YAML directly for advanced fields (e.g. custom `DagNode.overrides`), then run `testrix --check`. |

- **`--spec-json` / `--spec-file` example**:
  ```bash
  testrix item create --project shop-api --id req-create-order --name "Create Order" --order 2 \
    --method POST --url "{{baseUrl}}/orders" \
    --spec-json '{"headers":[{"name":"Content-Type","value":"application/json","enabled":true}],"body":{"type":"raw","language":"json","content":"{\"sku\":\"A1\"}"}}'
  ```
- **End-to-end Package & DAG recipe**:
  ```bash
  testrix folder create --project shop-api --name "Checkout"
  testrix item create --project shop-api --folder Checkout --id req-login --name "Login" --method POST --url "{{baseUrl}}/login"
  testrix item create --project shop-api --folder Checkout --id req-order --name "Order" --method POST --url "{{baseUrl}}/orders"
  testrix package create --name "Checkout Flow"
  testrix package add-item --package "Checkout Flow" --project shop-api --item req-login
  testrix package dag add-node --package "Checkout Flow" --id n1 --project shop-api --item req-login
  testrix package dag add-node --package "Checkout Flow" --id n2 --project shop-api --item req-order --needs n1
  testrix --check
  testrix package run --package "Checkout Flow" --fail-fast
  ```
- **Package examples**:
  ```bash
  testrix package list
  testrix package create --name "Smoke Suite"
  testrix package add-folder --package "Smoke Suite" --name "Auth"
  testrix package add-item --package "Smoke Suite" --project <project-ref> --item <item-ref> [--folder "Auth"]
  testrix package remove-item --package "Smoke Suite" --item <item-ref> [--folder "Auth"]
  testrix package run --package "Smoke Suite" [--env dev] [--var token=abc] [--fail-fast]
  testrix package delete --package "Smoke Suite"
  ```
- **Sharing examples**:
  ```bash
  testrix share status
  testrix share list
  testrix share publish --project <project-ref> [--channel server|r2]
  testrix share revoke --project <project-ref> [--channel server|r2]
  ```

## 3. Fallback: Manual YAML Editing & `--check`

Only edit YAML files directly if a specific operation is not covered by the CLI CRUD commands above (for example, authoring `DagNode.overrides` in `.testrix-sets.yaml`). If you must edit YAML manually:

1. **Locate the workspace** by reading `~/.testrix/config.yaml` (`workspaceDir`) or inspecting `testrix project list` output — never scan the disk.
2. **Read the target file** (and nearby `testrix.project.yaml`, `_folder.yaml`, or `environments/*.env.yaml` if inheritance or variables matter).
3. **Edit or create the YAML file** following the schema below. Preserve existing `id` values on existing files so open tabs, saved responses, and references do not break.
4. **Validate with `testrix --check` and notify the running GUI to reload** before finishing:

```bash
# Check the entire workspace (projects, folders, requests, environments, .testrix-sets.yaml, and DAGs)
testrix --check

# Notify the running Testrix GUI app to reload YAML changes immediately (returns guiRunning: true/false)
testrix notify reload

# Check one or more specific YAML files or a project directory
testrix --check path/to/file.request.yaml path/to/dev.env.yaml
testrix --check path/to/project-dir
```

- `testrix --check` on a workspace validates every YAML document **plus** `.testrix-sets.yaml` packages and DAG integrity (cycles, missing `needs`, unknown `requestId`), and prints counts (`projects`, `requests`, `environments`, `packages`, `dag nodes`).
- Exit code `0` (`OK ...` on stdout) means all checked files and DAGs passed validation.
- Non-zero exit code (`ERR ...` on stderr) means one or more checks failed; fix every reported error and re-run `testrix --check`.

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
- `request.postScript`: optional QuickJS script with `environment`, `request`, `response`, and `thread` globals (see [post-script.md](./post-script.md)).
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

### 4.6 `Package` & Execution DAG (`.testrix-sets.yaml`)

Packages (`RequestSet`) are stored in `<workspace-root>/.testrix-sets.yaml`. Use `testrix package ...` CLI commands to create/manage packages and items, and edit `.testrix-sets.yaml` (followed by `testrix notify reload`) to define or update a package's execution `dag`:

```yaml
sets:
  - id: set-checkout-flow
    name: Checkout Flow
    requests:
      - id: seti-login
        projectId: proj-starter
        requestId: req-login
        name: Login
        method: POST
        url: /auth/login
      - id: seti-create-order
        projectId: proj-starter
        requestId: req-create-order
        name: Create Order
        method: POST
        url: /orders
    dag:
      nodes:
        - id: node-login
          projectId: proj-starter
          requestId: req-login
          name: Login
          x: 60
          y: 120
        - id: node-create-order
          projectId: proj-starter
          requestId: req-create-order
          name: Create Order
          needs:
            - node-login
          overrides:
            headers:
              - name: Authorization
                value: "Bearer {{thread.authToken}}"
                enabled: true
            postScript: |
              thread.orderId = String(response.json().id);
          x: 340
          y: 120
```

#### Package DAG Rules for Agents

1. **Dependencies use `needs` on `DagNode`** (not a separate `edges` list): list upstream `DagNode.id` values in `node.needs`.
2. **No loops allowed**:
   - Every `dep` in `node.needs` must reference an existing `DagNode.id` in the same `dag.nodes`.
   - Self-loops (`node.id` in `node.needs`), duplicate entries in `needs`, and cycles of any length are invalid.
3. **`requests` list vs `dag.nodes`**: `dag.nodes` only needs `projectId` + `requestId` to execute; the package `requests` list is for the sidebar tree display and is optional for DAG execution (`package add-item` populates it for GUI visibility).
4. **Coordinates (`x`, `y`)**: optional (`0` by default); if all nodes are `(0, 0)`, Testrix auto-lays out the DAG left-to-right by topological layer.
5. **Float-layer `overrides`** (optional on `DagNode`):
   - Supported fields: `params`, `pathParams`, `headers`, `body`, `auth`, `postScript`.
   - **`overrides` does NOT support `url`**: to pass dynamic upstream values like `{{thread.id}}` in a URL per node, design the base request with query `params` (e.g. `/detail?id={{thread.id}}`) or `pathParams` (e.g. `/detail/{{id}}`), and override `params` or `pathParams` on the node.
   - Applied **only** during DAG execution without modifying the base `*.request.yaml` file.
6. **DAG-scoped `thread` variables & unset placeholders**:
   - Upstream `postScript` (base or `overrides.postScript`) can write `thread.<key> = value`.
   - Downstream nodes can reference `{{thread.<key>}}` in URLs, `params`, `pathParams`, `headers`, `body`, or `auth`, and read `thread.<key>` in downstream `postScript`.
   - If `{{thread.<key>}}` is **unset**, Testrix leaves the literal `{{thread.<key>}}` placeholder intact (it does **not** replace it with an empty string and does not fail template rendering).
   - `thread` is ephemeral and only exists for the duration of a single DAG run (and can be seeded via `--var key=value` in `testrix package run`).
7. **Failure & downstream blocking**:
   - Any node preparation error, HTTP transport error, HTTP status `>= 400`, or `postScript` exception marks that node as **failed**.
   - In the GUI and in `testrix package run --fail-fast`, the run stops immediately on the first failure (blocking all remaining nodes). Without `--fail-fast` in CLI, dependent downstream nodes are marked `skipped` while independent branches continue.
8. **Validate & run**:
   ```bash
   testrix package dag validate --package "Checkout Flow"
   testrix package run --package "Checkout Flow" --fail-fast
   testrix notify reload
   ```

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
