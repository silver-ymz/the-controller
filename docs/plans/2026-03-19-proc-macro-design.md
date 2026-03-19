# Proc-Macro: `#[derive_handlers]` Design

**Date:** 2026-03-19
**Issue:** [#64](https://github.com/silver-ymz/the-controller/issues/64)
**Status:** Approved

## Problem

`commands.rs` (2274 lines) and `server/main.rs` (2061 lines) are parallel wiring layers that each wrap every `service::*` function with near-identical boilerplate:

- `commands.rs`: maps `AppError → String`, wraps blocking calls with `tauri_blocking!`
- `server/main.rs`: maps `AppError → (StatusCode, String)`, wraps blocking calls with `spawn_blocking_handler!`, and defines a matching `#[derive(Deserialize)]` request struct per endpoint

A proc-macro attribute eliminates both layers from a single annotation on the service function.

## Scope

**Phase A (this plan):** PoC — migrate 3 service functions to validate the macro.
**Phase B (future):** Full migration of all ~50+ endpoints.

## Crate Structure

```
src-tauri/
  Cargo.toml                        ← add path dep on the-controller-macros
  the-controller-macros/
    Cargo.toml                      ← proc-macro = true
    src/
      lib.rs                        ← #[derive_handlers] entry point
      parse.rs                      ← syn AST parsing + validation
      tauri_gen.rs                  ← Tauri command token generation
      axum_gen.rs                   ← Axum handler + request struct generation
```

### `the-controller-macros/Cargo.toml`

```toml
[lib]
proc-macro = true

[dependencies]
syn   = { version = "2", features = ["full"] }
quote = "1"
proc-macro2 = "1"
```

## Macro API

```rust
use the_controller_macros::derive_handlers;

// Non-blocking service function
#[derive_handlers(tauri_command, axum_handler)]
pub fn create_project(state: &AppState, name: &str, repo_path: &str) -> Result<Project, AppError> {
    // original logic unchanged
}

// Blocking service function
#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn delete_project(state: &AppState, id: Uuid, delete_repo: bool) -> Result<(), AppError> {
    // original logic unchanged
}
```

### Argument Flags

| Flag | Effect |
|------|--------|
| `tauri_command` | Generate a Tauri IPC command |
| `axum_handler` | Generate an Axum HTTP handler + request struct (behind `#[cfg(feature = "server")]`) |
| `blocking` | Wrap the service call with `spawn_blocking` on both sides |

## Code Generation

### Non-blocking example: `create_project`

**Input:**
```rust
pub fn create_project(state: &AppState, name: &str, repo_path: &str) -> Result<Project, AppError>
```

**Generated Tauri command:**
```rust
#[tauri::command]
pub fn tauri_create_project(
    state: tauri::State<'_, AppState>,
    name: String,
    repo_path: String,
) -> Result<Project, String> {
    create_project(&state, &name, &repo_path).map_err(|e| e.to_string())
}
```

**Generated Axum handler + request struct:**
```rust
#[cfg(feature = "server")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub name: String,
    pub repo_path: String,
}

#[cfg(feature = "server")]
pub async fn axum_create_project(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::server::ServerState>>,
    axum::Json(req): axum::Json<CreateProjectRequest>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let result = create_project(&state.app, &req.name, &req.repo_path)
        .map_err(<(axum::http::StatusCode, String)>::from)?;
    crate::server::ok_json(result)
}
```

### Blocking example: `delete_project`

**Generated Tauri command:**
```rust
#[tauri::command]
pub async fn tauri_delete_project(
    state: tauri::State<'_, AppState>,
    project_id: String,
    delete_repo: bool,
) -> Result<(), String> {
    let id = crate::commands::parse_uuid(&project_id)?;
    let storage = state.storage.clone();
    let pty_manager = state.pty_manager.clone();
    tauri::async_runtime::spawn_blocking(move || {
        delete_project(&storage, &pty_manager, id, delete_repo).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Task failed: {e}"))?
}
```

**Generated Axum handler:**
```rust
#[cfg(feature = "server")]
pub async fn axum_delete_project(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::server::ServerState>>,
    axum::Json(req): axum::Json<DeleteProjectRequest>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let id = crate::server::parse_uuid(&req.project_id)?;
    let storage = state.app.storage.clone();
    let pty_manager = state.app.pty_manager.clone();
    let result = tokio::task::spawn_blocking(move || {
        delete_project(&storage, &pty_manager, id, req.delete_repo)
            .map_err(<(axum::http::StatusCode, String)>::from)
    })
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Task failed: {e}")))?;
    result.and_then(crate::server::ok_json)
}
```

## Parameter Mapping Rules

| Service parameter type | Tauri parameter | Request struct field | Notes |
|---|---|---|---|
| `&AppState` | `State<'_, AppState>` | skipped | injected from state extractor |
| `&str` | `String` | `String` | passed as `&arg` |
| `String` | `String` | `String` | |
| `Uuid` | `String` | `String` | call `parse_uuid(&req.field)?` |
| `bool` | `bool` | `bool` | |
| `Option<T>` | `Option<T>` | `Option<T>` | |
| `u16` / `u64` / `i64` | same | same | |

## PoC Migration Targets

| Function | Blocking | Covers |
|---|---|---|
| `list_projects` | No | simplest case, no UUID, no extra args |
| `create_project` | No | `&str` parameter mapping |
| `delete_project` | Yes | blocking path, `Uuid` mapping |

During Phase A, the generated `tauri_*` and `axum_*` symbols coexist with the existing hand-written wrappers. The existing `commands.rs` and `server/main.rs` are not modified. Validation is through `cargo build` and `cargo test`.

## Co-existence Strategy

Generated functions are prefixed (`tauri_`, `axum_`) to avoid name collisions with existing wrappers during the PoC phase. In Phase B (full migration), the existing hand-written functions are deleted and `invoke_handler` / router registrations are updated to use the generated names.

## Testing Strategy

1. **Compile test:** `cargo build --features server` must succeed with the macro applied to all 3 PoC functions
2. **`cargo expand`:** manually inspect generated token output for correctness
3. **`trybuild` tests** in `the-controller-macros/tests/`: verify expected expansions and error messages for malformed inputs
4. **Existing test suite:** `cargo test` and `pnpm test` must pass with no regressions

## Limitations (Phase A)

- Blocking field cloning is hardcoded to `storage`, `pty_manager`, `emitter` — not inferred from the function signature
- No support for `AppHandle`, `Option<String>` with defaults, or WebSocket handlers (these stay hand-written)
- No proc-macro IDE support (rust-analyzer may show false positives until the crate is properly indexed)

## Future Work (Phase B)

- Extend parameter mapping to cover all remaining types used across ~50 endpoints
- Auto-register routes into the Axum router via an `inventory`-style registry or a `generate_router!` macro
- Add `path` / `method` arguments to `axum_handler` for explicit route control
