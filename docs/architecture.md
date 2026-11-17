# Testrix Architecture

Testrix follows a four-crate layered Rust workspace designed for fast incremental builds, strict error-boundary separation, and clean GPUI Kit state ownership.

## Crate Dependency Graph

```text
testrix-app (GPUI Kit Desktop UI, anyhow, tracing)
  ├── testrix-http (reqwest + tokio + CancellationToken, thiserror)
  │     └── testrix-core
  ├── testrix-storage (yaml_serde + atomic disk persistence, thiserror)
  │     └── testrix-core
  └── testrix-core (domain models, auth inheritance, {{var}} interpolation, search)
```

Dependencies point strictly downward; `testrix-core` has zero UI, filesystem, or network dependencies.

## Key Design Decisions

### 1. Cascading Auth Inheritance (`Project -> Folder -> Subfolder -> Request`)
- `AuthConfig::Inherit` is the default on folders and requests.
- `resolve_request_doc_auth` and `resolve_folder_chain_auth` in `testrix-core` walk from the target node upward to the project root, stopping at the first node whose auth is not `Inherit` (`None`, `Bearer`, `Basic`, or `ApiKey`).
- `ResolvedAuth` preserves every step of the chain so the Auth tab can explain where the active credential came from and jump directly to the parent Folder or Project tab.

### 2. YAML-First Persistence (`testrix.io/v1`)
- Every HTTP request is stored as an independent `*.request.yaml` document (`docs/yaml-spec.md`).
- Writes go through `write_yaml_atomic` (write temp file in same directory, then `rename`) so partial writes cannot corrupt existing YAML documents.
- `RequestTabView` and `ContainerTabView` render a live read-only YAML preview synchronized from the active draft state.

### 3. GPUI & Tokio Isolation
- GPUI runs its UI event loop on `smol` / platform dispatch, while `reqwest` requires a Tokio reactor.
- `HttpRunner` owns a dedicated multi-thread Tokio runtime and dispatches requests onto it, returning a `tokio::sync::oneshot::Receiver` awaited inside `cx.spawn_in(window, ...)`.
- Each execution increments a monotonic `request_revision` and carries a `CancellationToken` so cancelling or re-sending discards stale responses safely.

### 4. Retained GPUI State Ownership
- Top-level `TestrixApp` owns the `WorkspaceTree`, open `WorkbenchTab` entities, active environment profile selector, `EnvEditorView` sheet content, and `ExplorerPanel`.
- Individual `RequestTabView`, `ContainerTabView`, `AuthEditor`, `KeyValueTable`, and `MultipartTable` entities retain their own `InputState`, `TextareaState`, `EditorState`, and `SelectState` handles across frames; `render` methods remain declarative and side-effect-free.
