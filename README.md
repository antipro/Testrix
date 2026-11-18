# Testrix

A native desktop API client and YAML-backed request workbench built with **Rust** (`1.100.0`) and **GPUI Kit (`v0.10.0`)**.

![Testrix Screenshot](docs/Screenshot.png)

## Highlights & Features

1. **Top-right Profile & Environment Variables**: Active profile selector (`Development`, `Staging`, `Production`, `No Environment`) and an `Env Vars` drawer (`Cmd+E` / `Ctrl+E`) supporting `{{variable}}` interpolation and masked secret variables.
2. **Right-side Project & Request Explorer with Quick Search**: Hierarchical explorer (`Project -> Folder -> Subfolder -> HTTP Request`) with live filtering across request names, HTTP methods, and URLs (`Cmd+K` / `Ctrl+K`).
3. **Import from OpenAPI, Postman, and Apifox**: Consolidated **Import** dropdown in the Explorer supporting **OpenAPI 3.0** (YAML/JSON), **Postman** collections & environments (`.zip`), and native **Apifox** JSON exports.
4. **Center Tabbed Workbench**: Open multiple HTTP requests, Project settings, and Folder settings as tabs with dirty indicators, method badges, tab context menu (with save confirmation on close), session persistence (remembers open tabs and last responses), and resizable Request / Response split panes.
5. **Full HTTP Request & Body Support**: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS` with toggleable Params (with automatic URL query parameter extraction), Headers, and Body modes (`None`, `JSON`, `Raw` Text/XML/HTML/JS/YAML, `x-www-form-urlencoded`, `Multipart` text + file, and `Binary`), plus response format auto-detection and manual syntax highlight switching.
6. **Post-Scripts (`gpui-shell` / QuickJS)**: Write JavaScript in the **Post-Script** tab (`request.postScript`) to run automatically after receiving a response—inspect `response` / `request` and mutate `environment.var` with automatic persistence to `*.env.yaml`. See [docs/post-script.md](docs/post-script.md).
7. **Request Code Generation**: Convert any materialized request into ready-to-run snippets in the **Code** tab for **cURL**, **cURL (Win)**, **wget**, and **PowerShell** (`Invoke-WebRequest`), with one-click copy.
8. **Cascading Auth Inheritance (`Project -> Folder -> Request`)**: Configure `Bearer Token`, `Basic Auth`, `API Key`, `No Auth`, or `Inherit from parent` at any level, with live inheritance-chain inspection and one-click parent navigation.
9. **Project Sharing & Active Shares**: Publish read-only API documentation directly from a Project's **Share** tab and manage or revoke active shares from the **Shares** button in the top toolbar across three channels:
   - **Local Network**: Built-in LAN HTTP viewer bound to a selected local network adapter IP.
   - **Testrix Server (`optional/node-web`)**: Self-hosted Express sharing server with account login, optional SMTP email verification, and per-project access tokens.
   - **Cloudflare Pages + R2 (`optional/cloudflare`)**: Zero-dependency Cloudflare Pages + R2 private API docs viewer. See [optional/cloudflare/README.md](optional/cloudflare/README.md).
10. **Preferences & 13-Language UI (i18n)**: Configure workspace directory (`~/.testrix/config.yaml`), session persistence, sharing credentials/access tokens, theme, and UI language (`English`, `Français`, `Deutsch`, `Español`, `Italiano`, `Português`, `Русский`, `العربية`, `Türkçe`, `日本語`, `한국어`, `简体中文`, `繁體中文`).
11. **YAML-First Storage (`testrix.io/v1`)**: Every HTTP request is stored on disk as `*.request.yaml` alongside `testrix.project.yaml`, `_folder.yaml`, and `environments/*.env.yaml`. See [docs/yaml-spec.md](docs/yaml-spec.md).

## Usage

### 1. Launch Testrix

This repository pins Rust `1.100.0` via [`rust-toolchain.toml`](rust-toolchain.toml). If your shell `PATH` has another `rustc` ahead of rustup, prepend rustup's shim bin directory (e.g. `/opt/homebrew/opt/rustup/bin`):

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

# Run Testrix (seeds ~/.testrix/workspace on first launch)
cargo run -p testrix-app

# Or point to a custom workspace directory
cargo run -p testrix-app -- --workspace /path/to/workspace
```

### 2. Core Workflows

- **Create or Import APIs**: Use the right-side **Explorer** to create Projects, Folders, and Requests, or click **Import** to bring in an **OpenAPI 3.0** spec, **Postman `.zip`**, or **Apifox JSON** export.
- **Configure Environments**: Select an active profile in the top-right dropdown and click **Env Vars** (`Cmd+E` / `Ctrl+E`) to edit `{{variable}}` values and secret variables.
- **Send Requests & Inspect Responses**: Choose an HTTP method, enter a URL (or use `{{base_url}}`), configure Params/Headers/Body/Auth, and press **Send** (`Cmd+Enter` / `Ctrl+Enter`).
- **Automate with Post-Scripts**: Open a request's **Post-Script** tab to extract tokens or values from `response.json()` into `environment.var["key"]` for subsequent requests.
- **Export CLI Snippets**: Open a request's **Code** tab, pick `cURL`, `cURL (Win)`, `wget`, or `PowerShell`, and copy the generated command.
- **Share a Project**:
  - Open a Project tab -> **Share** -> select **Local Network**, **Testrix Server**, or **Cloudflare R2 Storage**, then publish/upload.
  - Click **Shares** in the top toolbar at any time to view or revoke active shares.
  - Configure Testrix Server login, Cloudflare R2 credentials, and optional per-project access tokens in **Preferences**.

### 3. Keyboard Shortcuts

| Shortcut (macOS / Win & Linux) | Action |
|---|---|
| `Cmd+Enter` / `Ctrl+Enter` | Send active request |
| `Cmd+S` / `Ctrl+S` | Save active tab |
| `Cmd+K` / `Ctrl+K` | Focus Explorer quick search |
| `Cmd+N` / `Ctrl+N` | New HTTP request |
| `Cmd+W` / `Ctrl+W` | Close active tab |
| `Cmd+E` / `Ctrl+E` | Toggle Environment Variables drawer |
| `Cmd+Q` / `Ctrl+Q` | Quit Testrix |

### 4. Optional Self-Hosted Sharing Servers

- **Node/Express Server (`optional/node-web`)**:
  ```bash
  cd optional/node-web
  npm install
  cp .env.example .env   # Configure PORT, SESSION_SECRET, and SMTP/REQUIRE_EMAIL_VERIFICATION
  npm start
  ```
- **Cloudflare Pages + R2 (`optional/cloudflare`)**: Follow the deployment and R2 binding steps in [optional/cloudflare/README.md](optional/cloudflare/README.md).

## Workspace Architecture

- `crates/testrix-core` — Pure domain models, auth inheritance resolution, `{{var}}` interpolation, request materialization, codegen, OpenAPI/Postman/Apifox importers, and explorer filtering (`thiserror`).
- `crates/testrix-storage` — Atomic YAML persistence, directory scanner, validation, and starter workspace seeding (`yaml_serde`, `thiserror`).
- `crates/testrix-http` — Async HTTP execution engine on `reqwest` + `tokio` with `CancellationToken` support (`thiserror`).
- `crates/testrix-app` — GPUI Kit desktop application (`testrix` binary), `gpui-shell` post-script runner, local network share server, i18n, and UI integration tests (`anyhow`, `tracing`).

## Development & Checks

```bash
# Run all workspace unit, HTTP, storage, and GPUI headless UI integration tests
cargo test --workspace

# Run strict clippy & formatting checks
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```
