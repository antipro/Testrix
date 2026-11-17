# Testrix YAML Workspace & HTTP Request Specification (`testrix.io/v1`)

Version: `1.0.0`  
Schema API Version: `testrix.io/v1`

## 1. Purpose and Design Goals

Testrix stores every API project, folder, environment profile, and HTTP request as plain, human-readable, Git-friendly YAML documents on disk.

Design principles:
1. **One file per HTTP request**: Each HTTP request is an independent `.request.yaml` document so code reviews, merge conflicts, and history diffs remain scoped to single endpoints.
2. **Directory-mapped hierarchy**: Folders map directly to filesystem directories. Folder metadata and folder-level auth live in `_folder.yaml`.
3. **Explicit document envelope**: Every YAML file carries `apiVersion`, `kind`, `id`, and `name`.
4. **Cascading authentication inheritance**: Authentication settings cascade from **Project -> Folder -> Subfolder(s) -> HTTP Request**.
5. **Environment variable interpolation**: URLs, headers, query parameters, auth credentials, and request bodies support `{{variable_name}}` template placeholders resolved against the active Environment Profile.

---

## 2. Workspace Directory Layout

A Testrix workspace directory contains one or more project subdirectories:

```text
<workspace-root>/
├── .testrix-state.yaml                 # Optional local workspace UI state (active project & profile)
└── <project-slug>/
    ├── testrix.project.yaml            # Required project manifest (kind: Project)
    ├── environments/
    │   ├── development.env.yaml        # Environment profile (kind: Environment)
    │   ├── staging.env.yaml
    │   └── production.env.yaml
    ├── health-check.request.yaml       # Root-level request (kind: HttpRequest)
    └── users/
        ├── _folder.yaml                # Optional folder manifest (kind: Folder)
        ├── list-users.request.yaml     # Request inside folder (kind: HttpRequest)
        ├── create-user.request.yaml
        └── admin/
            ├── _folder.yaml            # Nested folder manifest (kind: Folder)
            └── audit-logs.request.yaml
```

### File Naming Rules

| Document Kind | Required Filename Pattern | Location |
|---|---|---|
| `Project` | `testrix.project.yaml` | `<project-root>/` |
| `Folder` | `_folder.yaml` | Inside any folder directory under `<project-root>/` |
| `HttpRequest` | `*.request.yaml` | `<project-root>/` or any nested folder directory |
| `Environment` | `*.env.yaml` | `<project-root>/environments/` |

- Hidden directories/files starting with `.` (except `.testrix-state.yaml` at workspace root) are ignored during scanning.
- A directory inside a project that does not yet contain `_folder.yaml` is treated as an implicit folder whose `name` defaults to the directory name, `id` is derived deterministically from its relative path, and `auth` defaults to `{ type: inherit }`.
- Slugs should use lowercase ASCII `[a-z0-9-]`.

---

## 3. Common Envelope Fields

Every `testrix.io/v1` document starts with these fields:

| Field | Type | Required | Description |
|---|---|---|---|
| `apiVersion` | `string` | Yes | Must be `"testrix.io/v1"`. |
| `kind` | `string` | Yes | One of `"Project"`, `"Folder"`, `"HttpRequest"`, `"Environment"`. |
| `id` | `string` | Yes | Stable non-empty domain identifier unique within its project/workspace. |
| `name` | `string` | Yes | Human-readable display name shown in explorer and tabs. |
| `description` | `string` | No | Optional documentation or notes. |

---

## 4. Document Schemas

### 4.1 `Project` (`testrix.project.yaml`)

Defines project-wide defaults, base URL, default headers, and root-level authentication inherited by folders and requests.

```yaml
apiVersion: testrix.io/v1
kind: Project
id: proj-starter
name: Starter Platform API
description: Core REST endpoints with project-level Bearer authentication
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

| Field | Type | Default | Description |
|---|---|---|---|
| `baseUrl` | `string` | `""` | Optional base URL prepended when a request URL starts with `/`. Supports `{{var}}`. |
| `auth` | `AuthConfig` | `{ type: none }` | Root auth configuration inherited by folders and requests. If set to `inherit`, evaluates to `none`. |
| `defaultHeaders` | `KeyValueRow[]` | `[]` | Default headers applied to all requests in the project unless overridden by request headers. |

---

### 4.2 `Folder` (`_folder.yaml`)

Defines folder ordering, description, and optional folder-level authentication overrides.

```yaml
apiVersion: testrix.io/v1
kind: Folder
id: fld-admin
name: Admin Operations
description: Privileged endpoints requiring an admin API key
order: 20
auth:
  type: apiKey
  key: X-Admin-Key
  value: "{{admin_key}}"
  in: header
```

| Field | Type | Default | Description |
|---|---|---|---|
| `order` | `i32` | `0` | Sort weight among sibling folders (ascending, tie-broken by `name`). |
| `auth` | `AuthConfig` | `{ type: inherit }` | Folder auth policy (`inherit`, `none`, `bearer`, `basic`, or `apiKey`). |

---

### 4.3 `HttpRequest` (`*.request.yaml`)

Defines a single executable HTTP request.

```yaml
apiVersion: testrix.io/v1
kind: HttpRequest
id: req-users-create
name: Create User
description: Provision a new account and return its assigned identifier.
order: 10
request:
  method: POST
  url: "{{base_url}}/post"
  params:
    - name: source
      value: desktop
      enabled: true
      description: Client origin tag
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
        "username": "{{sample_user}}",
        "role": "developer"
      }
  settings:
    timeoutMs: 30000
    followRedirects: true
    verifyTls: true
```

#### `request` Object Fields

| Field | Type | Default | Description |
|---|---|---|---|
| `method` | `HttpMethod` | `GET` | `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS` |
| `url` | `string` | `""` | Target URL or `/path` relative to `Project.baseUrl`. Supports `{{var}}`. |
| `params` | `KeyValueRow[]` | `[]` | Query parameters appended to the URL at execution time. |
| `headers` | `KeyValueRow[]` | `[]` | Request headers. Applied after `Project.defaultHeaders` and Auth headers. |
| `auth` | `AuthConfig` | `{ type: inherit }` | Request authentication configuration. |
| `body` | `RequestBody` | `{ type: none }` | Payload configuration (discriminated by `type`). |
| `settings` | `RequestSettings` | `{ 30000, true, true }` | Per-request execution options. |

#### `KeyValueRow`

```yaml
name: Content-Language
value: en-US
enabled: true
description: Preferred response language
```

- Disabled rows (`enabled: false`) are preserved in YAML and in the editor UI but excluded during request execution.

#### `RequestBody` Variants (Discriminated by `type`)

1. **`none`**: No request payload.
   ```yaml
   body:
     type: none
   ```

2. **`json`**: JSON payload (`Content-Type: application/json` added automatically if not explicitly set).
   ```yaml
   body:
     type: json
     content: '{"hello": "world"}'
   ```

3. **`raw`**: Raw text payload with explicit format (`text`, `xml`, `html`, `javascript`, `yaml`).
   ```yaml
   body:
     type: raw
     rawFormat: xml
     content: "<ping/>"
   ```

4. **`formUrlEncoded`**: `application/x-www-form-urlencoded` form fields (`KeyValueRow[]`).
   ```yaml
   body:
     type: formUrlEncoded
     fields:
       - name: grant_type
         value: client_credentials
         enabled: true
   ```

5. **`multipart`**: `multipart/form-data` fields supporting both text and file parts.
   ```yaml
   body:
     type: multipart
     fields:
       - name: metadata
         fieldType: text
         value: '{"env":"{{env_name}}"}'
         enabled: true
       - name: artifact
         fieldType: file
         value: ./payload.bin
         fileName: payload.bin
         contentType: application/octet-stream
         enabled: true
   ```

6. **`binary`**: Binary file upload (`application/octet-stream` unless overridden).
   ```yaml
   body:
     type: binary
     filePath: ./archive.tar.gz
     contentType: application/gzip
   ```

#### `RequestSettings`

| Field | Type | Default | Description |
|---|---|---|---|
| `timeoutMs` | `u64` | `30000` | Request timeout in milliseconds (`> 0`). |
| `followRedirects` | `bool` | `true` | Automatically follow HTTP 3xx redirects. |
| `verifyTls` | `bool` | `true` | Verify server TLS certificates. |

---

### 4.4 `Environment` (`environments/*.env.yaml`)

Defines an environment profile and its variables (shown in the top-right profile manager).

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
    description: Local or sandbox base URL
  - key: api_token
    value: dev-token-9f21a
    enabled: true
    secret: true
    description: Development Bearer token
```

| Field | Type | Default | Description |
|---|---|---|---|
| `variables` | `EnvVariable[]` | `[]` | List of `{ key, value, enabled, secret, description }`. |

- When `secret: true`, UI inputs mask the value by default while still resolving it during HTTP execution.
- If duplicate enabled keys exist in one environment, the last enabled entry wins.

---

## 5. Authentication & Inheritance Semantics

### 5.1 `AuthConfig` Variants (Discriminated by `type`)

| `type` | Additional Fields | Valid On |
|---|---|---|
| `inherit` | — | `Folder`, `HttpRequest` |
| `none` | — | `Project`, `Folder`, `HttpRequest` |
| `bearer` | `token: string`, `prefix?: string` (default `"Bearer"`) | `Project`, `Folder`, `HttpRequest` |
| `basic` | `username: string`, `password: string` | `Project`, `Folder`, `HttpRequest` |
| `apiKey` | `key: string`, `value: string`, `in: header \| query` | `Project`, `Folder`, `HttpRequest` |

### 5.2 Resolution Algorithm

Given a target node $N_0$ (`HttpRequest` or `Folder`) with ancestor chain $[N_0, N_1, \dots, N_k]$ where $N_k$ is the enclosing `Project`:

1. Inspect $N_i.\text{auth}$ in order $i = 0, 1, \dots, k$.
2. The first node whose `auth.type != inherit` becomes the **Effective Auth Source**.
3. If that node is `none`, effective auth is **No Auth** (explicitly terminating inheritance from higher ancestors).
4. If every node up to and including `Project` is `inherit` or `none`, effective auth is **No Auth**.
5. Before sending an HTTP request, all string fields inside the effective `AuthConfig` undergo `{{variable}}` interpolation against the active `Environment`.

### 5.3 Precedence When Materializing Requests

1. **URL resolution**:
   - Interpolate `{{var}}` in `request.url`.
   - If the resulting URL starts with `/` and `Project.baseUrl` is non-empty, prepend the interpolated `Project.baseUrl` (stripping trailing `/` to avoid `//`).
   - Append enabled `request.params`.
2. **Headers**:
   - Start with enabled `Project.defaultHeaders` (interpolated).
   - Apply body default `Content-Type` (for `json`, `raw`, `formUrlEncoded`, `binary`) if not already present.
   - Apply effective `AuthConfig` (`Authorization` header, API key header, or API key query param).
   - Apply enabled `request.headers` (interpolated). Explicit request headers with the same case-insensitive name override inherited/default headers.

---

## 6. Validation Rules

1. `apiVersion` must equal `"testrix.io/v1"`.
2. `kind` must match the file role (`testrix.project.yaml` -> `Project`, `_folder.yaml` -> `Folder`, `*.request.yaml` -> `HttpRequest`, `*.env.yaml` -> `Environment`).
3. `id` and `name` must be non-empty after trimming whitespace.
4. `settings.timeoutMs` must be `>= 1`.
5. `Project.auth` resolves `inherit` as `none` if encountered in legacy files, though `none` is canonical at the project root.
