# Phase B: Full `#[derive_handlers]` Migration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task.

**Goal:** Migrate all ~80 Tauri commands and Axum handlers to use `#[derive_handlers]`, eliminating ~4000 lines of hand-written boilerplate from `commands.rs` and `server/main.rs`.

**Architecture:** (1) Make `AppState` cloneable so the `blocking` macro flag can clone it into `spawn_blocking` closures. (2) Refactor ~20 service functions that take individual `Arc` fields to take `&AppState` instead. (3) Extend the macro to support `async`, `blocking`, and additional parameter types. (4) Annotate all service functions, switch routing, delete old wrappers.

**Tech Stack:** Rust proc-macros (`syn 2`, `quote 1`), Tauri 2, Axum 0.8

---

### Functions excluded from migration (keep hand-written)

These require special handling that the macro cannot express:

| Function | Reason |
|---|---|
| `copy_image_file_to_clipboard` | No service function; uses `AppHandle` for clipboard access |
| `capture_app_screenshot` | No service function; uses `AppHandle` for window handle |
| `stage_session` (server) | Returns `501 NOT_IMPLEMENTED` stub |
| `merge_session_branch` (server) | Has `tokio::time::timeout` wrapper with GATEWAY_TIMEOUT |
| `list_directories_at` (server) | Calls `list_directories_at_safe` (different function for security) |
| `connect_session` (tauri) | Custom structured tracing on join error |
| `ws_upgrade` / `handle_ws` | WebSocket upgrade handler, not a command |

---

### Task 1: Make `AppState` cloneable

**Why:** The `blocking` macro flag needs to clone `AppState` into a `spawn_blocking` closure. Currently, 4 fields are not `Arc`-wrapped: `secure_env_request`, `staging_lock`, `frontend_log`, `voice_generation`.

**Files:**
- Modify: `src-tauri/src/state.rs`

**Changes to `AppState` struct:**

```rust
// Change these 4 fields from direct wrapping to Arc wrapping:
pub(crate) secure_env_request: Arc<Mutex<Option<crate::secure_env::ActiveSecureEnvRequest>>>,
pub staging_lock: Arc<TokioMutex<()>>,
pub frontend_log: Arc<std::sync::Mutex<Option<std::fs::File>>>,
pub voice_generation: Arc<AtomicU64>,
```

Add `#[derive(Clone)]` to `AppState`.

Update `AppState::from_storage()` to wrap these fields in `Arc::new(...)`.

**Then update all call sites** (use `grep -rn` to find them):
- `state.secure_env_request.lock()` → no change needed (Arc auto-derefs)
- `state.staging_lock.lock()` → no change needed
- `state.frontend_log.lock()` → no change needed
- `state.voice_generation.fetch_add(...)` → no change needed
- `voice_generation: AtomicU64::new(0)` in test code → `voice_generation: Arc::new(AtomicU64::new(0))`

**Verify:** `cd src-tauri && cargo test`
**Commit:** `refactor: make AppState cloneable by Arc-wrapping remaining fields`

---

### Task 2: Refactor service/notes.rs — take `&AppState`

**Why:** All 14 note service functions take `&Arc<Mutex<Storage>>` as first param. Change to `&AppState`.

**Files:**
- Modify: `src-tauri/src/service/notes.rs`
- Modify: `src-tauri/src/commands/notes.rs` (update callers)
- Modify: `src-tauri/src/server/main.rs` (update callers)

**Pattern (apply to all 14 functions):**

```rust
// BEFORE:
pub fn list_notes(storage: &Arc<Mutex<Storage>>, folder: &str) -> Result<Vec<NoteEntry>, AppError> {
    let storage = storage.lock().map_err(AppError::internal)?;
    ...
}

// AFTER:
pub fn list_notes(state: &AppState, folder: &str) -> Result<Vec<NoteEntry>, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    ...
}
```

**Functions to refactor:** `list_notes`, `read_note`, `write_note`, `create_note`, `rename_note`, `duplicate_note`, `delete_note`, `list_note_folders`, `create_note_folder`, `rename_note_folder`, `delete_note_folder`, `commit_pending_notes`, `save_note_image`, `resolve_note_asset_path`.

**Update callers in `commands/notes.rs`:** Change `service::list_notes(&storage, ...)` to `service::list_notes(&state_clone, ...)` where `state_clone` is a cloned `AppState` (from Task 1).

**Update callers in `server/main.rs`:** Change `service::list_notes(&state.app.storage, ...)` to `service::list_notes(&state.app, ...)`.

**Verify:** `cd src-tauri && cargo test`
**Commit:** `refactor: service/notes.rs takes &AppState instead of individual Arc fields`

---

### Task 3: Refactor service/sessions.rs — take `&AppState`

**Files:**
- Modify: `src-tauri/src/service/sessions.rs`
- Modify: `src-tauri/src/commands.rs` (update callers)
- Modify: `src-tauri/src/server/main.rs` (update callers)

**Functions to refactor:**
- `restore_sessions(storage: &Arc<Mutex<Storage>>)` → `restore_sessions(state: &AppState)`
- `connect_session(storage, pty_manager, emitter, ...)` → `connect_session(state: &AppState, ...)`
- `create_session(storage, pty_manager, emitter, ...)` → `create_session(state: &AppState, ...)`
- `get_session_commits(storage, ...)` → `get_session_commits(state: &AppState, ...)`
- `get_session_token_usage(storage, ...)` → `get_session_token_usage(state: &AppState, ...)`

Inside each function, replace `storage` with `&state.storage`, `pty_manager` with `&state.pty_manager`, `emitter` with `&state.emitter`.

**Update callers** the same way as Task 2.

**Verify:** `cd src-tauri && cargo test`
**Commit:** `refactor: service/sessions.rs takes &AppState instead of individual Arc fields`

---

### Task 4: Refactor service/auth.rs, config.rs, projects.rs — take `&AppState`

**Files:**
- Modify: `src-tauri/src/service/auth.rs`
- Modify: `src-tauri/src/service/config.rs`
- Modify: `src-tauri/src/service/projects.rs`
- Modify: `src-tauri/src/commands.rs` (update callers)
- Modify: `src-tauri/src/server/main.rs` (update callers)

**Functions to refactor:**
- `start_claude_login(pty_manager, emitter)` → `start_claude_login(state: &AppState)`
- `stop_claude_login(pty_manager, session_id)` → `stop_claude_login(state: &AppState, session_id)`
- `load_terminal_theme_blocking(storage)` → `load_terminal_theme_blocking(state: &AppState)`
- `generate_architecture(repo_path, emitter)` → `generate_architecture(state: &AppState, repo_path)`
- `delete_project(storage, pty_manager, ...)` → `delete_project(state: &AppState, ...)`

**Verify:** `cd src-tauri && cargo test`
**Commit:** `refactor: service auth/config/projects take &AppState`

---

### Task 5: Extend macro — `async` service function support

**Files:**
- Modify: `src-tauri/the-controller-macros/src/parse.rs`
- Modify: `src-tauri/the-controller-macros/src/tauri_gen.rs`
- Modify: `src-tauri/the-controller-macros/src/axum_gen.rs`

**Change in `parse.rs`:** Add `is_async: bool` field to `ParsedService`, populated from `item_fn.sig.asyncness.is_some()`.

**Change in `tauri_gen.rs`:** If `is_async`:
```rust
#[tauri::command]
pub async fn tauri_foo(state: ::tauri::State<'_, crate::state::AppState>, ...) -> Result<T, String> {
    foo(&state, ...).await.map_err(|e| e.to_string())
}
```
(Add `.await` after service call, function becomes `async fn`.)

**Change in `axum_gen.rs`:** If `is_async`, add `.await` after the service call. (Already `async fn`.)

**Verify:** `cd src-tauri/the-controller-macros && cargo check`
**Commit:** `feat(macros): add async service function support`

---

### Task 6: Extend macro — `blocking` flag support

**Files:**
- Modify: `src-tauri/the-controller-macros/src/lib.rs` (remove the blocking error)
- Modify: `src-tauri/the-controller-macros/src/tauri_gen.rs`
- Modify: `src-tauri/the-controller-macros/src/axum_gen.rs`

**In `lib.rs`:** Remove the `blocking` rejection error.

**In `tauri_gen.rs`:** When `blocking` flag is set, generate:
```rust
#[tauri::command]
pub async fn tauri_foo(
    state: ::tauri::State<'_, crate::state::AppState>,
    name: String,
) -> Result<T, String> {
    let state = (*state).clone();
    // (uuid parse stmts here, before the closure)
    ::tauri::async_runtime::spawn_blocking(move || {
        foo(&state, &name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Task failed: {e}"))?
}
```

**In `axum_gen.rs`:** When `blocking` flag is set, generate:
```rust
#[cfg(feature = "server")]
pub async fn axum_foo(
    ::axum::extract::State(state): ...,
    ::axum::Json(req): ...,
) -> Result<::axum::Json<::serde_json::Value>, (::axum::http::StatusCode, String)> {
    let app = state.app.clone();
    // (uuid parse stmts)
    ::tokio::task::spawn_blocking(move || {
        foo(&app, &req.name)
            .map_err(<(::axum::http::StatusCode, String)>::from)
    })
    .await
    .map_err(|e| (::axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Task failed: {e}")))?
    .and_then(crate::server_helpers::ok_json)
}
```

**Verify:** `cd src-tauri/the-controller-macros && cargo check`
**Commit:** `feat(macros): add blocking flag support`

---

### Task 7: Extend macro — additional parameter types

**Files:**
- Modify: `src-tauri/the-controller-macros/src/parse.rs`
- Modify: `src-tauri/the-controller-macros/src/tauri_gen.rs`
- Modify: `src-tauri/the-controller-macros/src/axum_gen.rs`

**New `ParamKind` variants:**

1. **`ByteSlice`** — for `&[u8]` params. In Tauri/request struct: `String`. Call with `arg.as_bytes()`. (Used by `write_to_pty`, `send_raw_to_pty`.)

2. **No-state functions** — when the function has NO `&AppState` param, omit the State extractor in Tauri and the `state.app` deref in Axum. The Axum handler still needs the `State(state)` param for the router, but doesn't use it. Alternatively, for truly no-state functions, the Axum handler can omit the state extractor entirely.

**Changes in `parse.rs`:**
- Add `ParamKind::ByteSlice` for `&[u8]`
- Update `classify_type()`: check for `&[u8]` (reference to slice)

**Changes in `tauri_gen.rs`:**
- `ByteSlice`: wrapper takes `String`, call with `arg.as_bytes()`
- No AppState: omit State param

**Changes in `axum_gen.rs`:**
- `ByteSlice`: request struct field is `String`, call with `req.field.as_bytes()`
- No AppState: still take State extractor (needed for router), pass `&state.app` only if used

**Verify:** `cd src-tauri/the-controller-macros && cargo check`
**Commit:** `feat(macros): add &[u8] param type and no-state function support`

---

### Task 8: Annotate ALL service functions with `#[derive_handlers]`

**Files:**
- Modify: `src-tauri/src/service/projects.rs`
- Modify: `src-tauri/src/service/sessions.rs`
- Modify: `src-tauri/src/service/github.rs`
- Modify: `src-tauri/src/service/maintainer.rs`
- Modify: `src-tauri/src/service/notes.rs`
- Modify: `src-tauri/src/service/config.rs`
- Modify: `src-tauri/src/service/auth.rs`
- Modify: `src-tauri/src/service/deploy.rs`
- Modify: `src-tauri/src/service/secure_env.rs`
- Modify: `src-tauri/src/service/voice.rs`

Add `use the_controller_macros::derive_handlers;` and annotate each function.

**Excluded functions** (keep hand-written or not applicable):
- `copy_image_file_to_clipboard`, `capture_app_screenshot` (no service fn)
- `stage_session` (server stub), `merge_session_branch` (server timeout)
- `list_directories_at` (server calls different fn)
- `connect_session` (tauri has custom error logging)
- `ws_upgrade` / `handle_ws` (WebSocket)
- Internal helpers (`validate_project_name`, `scaffold_project_blocking`, etc.)

**Flag selection guide:**
- Sync functions that don't need spawn_blocking → `#[derive_handlers(tauri_command, axum_handler)]`
- Sync functions that are blocking → `#[derive_handlers(tauri_command, axum_handler, blocking)]`
- Async functions → `#[derive_handlers(tauri_command, axum_handler)]` (macro detects `async`)
- Functions only on one side → `#[derive_handlers(tauri_command)]` or `#[derive_handlers(axum_handler)]`

**Verify:** `cd src-tauri && cargo check && cargo check --features server`
**Commit:** `feat: annotate all service functions with derive_handlers`

---

### Task 9: Switch Tauri `invoke_handler` to generated names

**Files:**
- Modify: `src-tauri/src/lib.rs` (the `invoke_handler` block)

Change each entry from `commands::foo` to `service::tauri_foo`:

```rust
.invoke_handler(tauri::generate_handler![
    service::tauri_restore_sessions,
    service::tauri_connect_session, // keep hand-written for custom error logging
    service::tauri_create_project,
    // ... etc
])
```

**Note:** For excluded functions, keep the existing `commands::*` path.

**Verify:** `cd src-tauri && cargo check`
**Commit:** `refactor: switch invoke_handler to macro-generated Tauri commands`

---

### Task 10: Switch Axum router to generated names

**Files:**
- Modify: `src-tauri/src/server/main.rs` (the `Router::new()` block)

Change each route handler from the local function name to `service::axum_foo`:

```rust
.route("/api/list_projects", post(service::axum_list_projects))
.route("/api/create_project", post(service::axum_create_project))
// ... etc
```

**Note:** For excluded functions, keep the existing hand-written handler.

**Verify:** `cd src-tauri && cargo check --features server`
**Commit:** `refactor: switch Axum router to macro-generated handlers`

---

### Task 11: Delete old wrappers

**Files:**
- Modify: `src-tauri/src/commands.rs` — delete all migrated `#[tauri::command]` functions
- Modify: `src-tauri/src/commands/notes.rs` — delete all migrated functions
- Modify: `src-tauri/src/server/main.rs` — delete all migrated handler functions and request structs
- Possibly delete: `src-tauri/src/commands/github.rs`, `src-tauri/src/commands/media.rs` (if empty)

Keep:
- `commands.rs`: `parse_uuid` helper, `tauri_blocking!` macro (might still be needed), `update_project_with_rollback` test helper, excluded functions
- `server/main.rs`: `spawn_blocking_handler!` macro, `ok_json`/`parse_uuid` are now in `server_helpers.rs`, excluded handler functions, auth middleware, WebSocket handler, `main()`, `run_server()`, router setup (with new paths)

**Verify:** `cd src-tauri && cargo test && cargo check --features server`
**Commit:** `refactor: delete hand-written wrappers replaced by derive_handlers`

---

### Task 12: Final verification

**Steps:**
1. `pnpm check` — frontend typecheck
2. `cd src-tauri && cargo fmt --check` — formatting
3. `cd src-tauri && cargo clippy -- -D warnings` — lints
4. `cd src-tauri && cargo clippy --features server -- -D warnings` — server lints
5. `cd src-tauri && cargo test` — all tests pass
6. `cd src-tauri && cargo expand --lib --features server service::projects 2>&1 | head -50` — spot-check expansion

Fix any issues found.

**Commit:** `style: fix lint and formatting issues`
