# Testrix Post-Script Specification (`gpui-shell`)

Every `HttpRequest` (`*.request.yaml`) supports an optional `request.postScript` JavaScript snippet that runs automatically after an HTTP response is received. Post-scripts execute inside an isolated `gpui-shell` (QuickJS) runtime.

---

## 1. YAML Schema

In a `*.request.yaml` file, `postScript` is an optional string under `request`:

```yaml
apiVersion: testrix.io/v1
kind: HttpRequest
id: req-login
name: Login
request:
  method: POST
  url: "{{base_url}}/auth/login"
  body:
    type: json
    content: |
      {
        "username": "{{sample_user}}",
        "password": "{{sample_password}}"
      }
  postScript: |
    if (response.status >= 200 && response.status < 300) {
      const data = JSON.parse(response.body);
      environment.var["api-token"] = data.token;
    }
```

You can also edit `postScript` directly in the **Post-Script** tab of any request in the Testrix UI.

---

## 2. Global Variables

When a post-script runs, Testrix injects three global objects: `environment`, `request`, and `response`.

### `environment`

Represents the currently active Environment Profile.

| Property | Type | Description |
|---|---|---|
| `environment.id` | `string` | Active profile ID (empty string `""` if no profile is active). |
| `environment.name` | `string` | Active profile display name (empty string `""` if no profile is active). |
| `environment.var` | `Record<string, string>` | Map of enabled variable keys to their string values in the active profile. |

Mutating `environment.var["<key>"] = "<value>"` (or adding a new key) updates the active profile in memory, persists the updated `*.env.yaml` file to disk, and immediately refreshes open request/project tabs so subsequent requests see the new value.

- Non-string values assigned to `environment.var[key]` are converted to strings (`JSON.stringify` / `toString()`).
- Assigning `null` or `undefined` is ignored for that key.
- If an existing variable was disabled, updating its value re-enables it.

### `request`

Read-only snapshot of the executed HTTP request (after `{{variable}}` interpolation).

| Property | Type | Description |
|---|---|---|
| `request.id` | `string` | Request document ID. |
| `request.name` | `string` | Request display name. |
| `request.method` | `string` | HTTP method (`"GET"`, `"POST"`, etc.). |
| `request.url` | `string` | Final resolved URL sent over the wire. |
| `request.headers` | `Record<string, string>` | Resolved request headers map. |

### `response`

Read-only snapshot of the HTTP response returned by the server.

| Property | Type | Description |
|---|---|---|
| `response.status` | `number` | HTTP status code (e.g. `200`, `401`). |
| `response.reason` | `string` | HTTP reason phrase (e.g. `"OK"`). |
| `response.headers` | `Record<string, string>` | Response headers map. |
| `response.body` | `string` | Raw response body text from the server. |
| `response.durationMs` | `number` | Request duration in milliseconds. |
| `response.sizeBytes` | `number` | Response body size in bytes. |
| `response.json()` | `() => any` | Convenience helper equivalent to `JSON.parse(response.body)`. |

---

## 3. Execution Lifecycle & Error Handling

1. Testrix sends the HTTP request and receives `HttpResponseSnapshot`.
2. If `request.postScript` is non-empty, Testrix creates an isolated `gpui-shell` runtime, populates `environment`, `request`, and `response`, and evaluates the script.
3. Even if the script throws partway through, any changes already written to `environment.var` are committed via a `finally` block.
4. If the script throws a JavaScript error (for example, `JSON.parse` on non-JSON output), Testrix displays the error in the **Post-Script** tab status badge and pushes a notification, without discarding the HTTP response.

---

## 4. Examples

### Extract a Bearer Token on Login

```javascript
if (response.status === 200) {
  const data = JSON.parse(response.body);
  if (data && data.token) {
    environment.var["api-token"] = data.token;
  }
}
```

### Extract Nested Attributes Using `response.json()`

```javascript
const payload = response.json();
environment.var["user-id"] = String(payload.data.user.id);
environment.var["session-id"] = payload.data.session.id;
```

### Read a Response Header

```javascript
const requestId = response.headers["x-request-id"];
if (requestId) {
  environment.var["last-request-id"] = requestId;
}
```
