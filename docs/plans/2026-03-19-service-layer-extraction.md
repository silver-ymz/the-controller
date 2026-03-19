# Service Layer Extraction Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use the-controller-executing-plans to implement this plan task-by-task.

**Goal:** Eliminate business logic duplication between `commands.rs` (Tauri desktop) and `server/main.rs` (Axum HTTP). Extract a shared service layer so both API surfaces become thin adapters over the same code.

**Problem:** `commands.rs` (~3800 lines) and `server/main.rs` (~3600 lines) duplicate the same business logic with only two differences: error type (`String` vs `(StatusCode, String)`) and parameter extraction (typed function args vs `Json<Value>`). Every new command must be written twice; every bug fix must be applied twice.

**Architecture:** Introduce `src/service.rs` containing all business logic as plain `async fn`s that accept `&AppState` and typed arguments, returning `Result<T, AppError>`. Tauri commands and Axum handlers become 1-5 line adapters that extract args, call the service, and map the result.

**Tech Stack:** Rust, Tauri v2, Axum 0.8

**Migration strategy:** Incremental. Each task migrates one category of commands. The existing `server_routes_cover_desktop_command_surface` test ensures no routes are lost. Each task is independently shippable.

---

## Task 0: Introduce `AppError` and `service` module skeleton

**Files:**
- Create: `src-tauri/src/error.rs`
- Create: `src-tauri/src/service.rs`
- Modify: `src-tauri/src/lib.rs`

**Step 1: Write the test**

Add a test in `error.rs` that:
- Asserts `AppError::BadRequest("x".into())` converts to `String` via `From<AppError>` (for Tauri)
- Asserts `AppError::NotFound("x".into())` converts to `(StatusCode, String)` with 404 (for Axum)
- Asserts `AppError::Internal("x".into())` converts to `(StatusCode, String)` with 500

**Step 2: Run test — expect FAIL** (types don't exist yet)

**Step 3: Implement**

```rust
// src/error.rs
use std::fmt;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(msg) | Self::NotFound(msg) | Self::Internal(msg) => f.write_str(msg),
        }
    }
}

// For Tauri commands: AppError → String
impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.to_string()
    }
}
```

For Axum, add a feature-gated impl:

```rust
#[cfg(feature = "server")]
impl From<AppError> for (axum::http::StatusCode, String) {
    fn from(e: AppError) -> (axum::http::StatusCode, String) {
        use axum::http::StatusCode;
        match e {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        }
    }
}
```

Add convenience conversions for common error sources:

```rust
impl AppError {
    /// Wrap a mutex poisoning or I/O error as Internal.
    pub fn internal(e: impl fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }
}
```

Create empty `src/service.rs` with a module doc comment explaining the pattern. Register both modules in `lib.rs`.

**Step 4: Run test — expect PASS**

**Step 5: Run `cargo clippy`, `cargo fmt --check`, `pnpm check`**

**Step 6: Commit**

```
feat: add AppError type and service module skeleton
```

---

## Task 1: Migrate project management commands (trivial batch)

**Commands:** `list_projects`, `check_onboarding`, `create_project`, `load_project`, `delete_project`, `get_agents_md`, `update_agents_md`

**Files:**
- Modify: `src-tauri/src/service.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/server/main.rs`

**Step 1: Write tests in `service.rs`**

Create `service::tests` module. For `list_projects`, verify it returns the inventory from a test `AppState`. For `create_project`, verify it rejects duplicate names with `AppError::BadRequest`. These tests exercise the service layer directly, independent of transport.

**Step 2: Run tests — expect FAIL**

**Step 3: Implement service functions**

Example pattern for each:

```rust
// src/service.rs
use crate::error::AppError;
use crate::state::AppState;
use crate::storage::ProjectInventory;

pub fn list_projects(state: &AppState) -> Result<ProjectInventory, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    storage.list_projects().map_err(AppError::internal)
}

pub fn create_project(state: &AppState, name: &str, repo_path: &str) -> Result<Project, AppError> {
    commands::validate_project_name(name).map_err(|e| AppError::BadRequest(e))?;
    // ... all business logic from commands.rs::create_project ...
}

pub async fn delete_project(state: &AppState, project_id: Uuid, delete_repo: bool) -> Result<(), AppError> {
    // ... logic from commands.rs::delete_project, using tokio::task::spawn_blocking ...
}
```

**Step 4: Rewire `commands.rs` to delegate**

```rust
#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Result<ProjectInventory, String> {
    crate::service::list_projects(&state).map_err(Into::into)
}

#[tauri::command]
pub fn create_project(state: State<AppState>, name: String, repo_path: String) -> Result<Project, String> {
    crate::service::create_project(&state, &name, &repo_path).map_err(Into::into)
}
```

**Step 5: Rewire `server/main.rs` to delegate**

Introduce typed request structs:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectArgs {
    name: String,
    repo_path: String,
}

async fn create_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<CreateProjectArgs>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project = service::create_project(&state.app, &args.name, &args.repo_path)
        .map_err(Into::into)?;
    Ok(Json(serde_json::to_value(project).unwrap()))
}
```

**Step 6: Run tests — expect PASS**

Run: `cd src-tauri && cargo test`
Run: `cd src-tauri && cargo test --features server`

**Step 7: Run lint gates**

**Step 8: Commit**

```
refactor: extract project management into service layer
```

---

## Task 2: Migrate session management commands

**Commands:** `restore_sessions`, `connect_session`, `create_session`, `close_session`, `write_to_pty`, `send_raw_to_pty`, `resize_pty`, `stage_session`, `unstage_session`

**Files:** Same three files as Task 1.

**Key complexity:** `create_session` is the most complex command (~200 lines) with worktree creation, PTY spawning, and rollback-on-failure. `connect_session` is medium. The rest are trivial delegations to `PtyManager`.

**Step 1: Write tests**

- `service::tests::create_session_rejects_bad_uuid` → `AppError::BadRequest`
- `service::tests::close_session_not_found` → `AppError::NotFound`

**Step 2: Implement service functions**

For `create_session`, move the entire body (worktree creation, PTY spawn, rollback logic) into `service::create_session()`. Both `commands.rs` and `server/main.rs` currently have independent implementations of this — unify them.

For trivial PTY operations (`write_to_pty`, `resize_pty`, etc.), the service function is ~3 lines:

```rust
pub fn write_to_pty(state: &AppState, session_id: Uuid, data: &str) -> Result<(), AppError> {
    let mut mgr = state.pty_manager.lock().map_err(AppError::internal)?;
    mgr.write_to_session(session_id, data).map_err(AppError::internal)
}
```

**Step 3-6:** Same pattern as Task 1 (rewire both sides, test, lint, commit).

```
refactor: extract session management into service layer
```

---

## Task 3: Migrate notes commands

**Commands:** `list_notes`, `read_note`, `write_note`, `create_note`, `delete_note`, `rename_note`, `duplicate_note`, `list_folders`, `create_folder`, `rename_folder`, `delete_folder`, `commit_notes`, `save_note_image`, `resolve_note_asset_path`, `send_note_ai_chat`

**Files:** Same three files, plus `src-tauri/src/commands/notes.rs`.

**Key insight:** The note commands already delegate to `notes::` module functions. The duplication is in the adapter layer (extracting `base_dir` from storage, calling the note function, doing `try_commit`). The `try_commit` pattern is duplicated between `commands/notes.rs` and `server/main.rs` — unify it in the service layer.

**Step 1-6:** Same TDD pattern.

```
refactor: extract notes commands into service layer
```

---

## Task 4: Migrate GitHub issue commands

**Commands:** `list_github_issues`, `list_assigned_issues`, `create_github_issue`, `generate_issue_body`, `post_github_comment`, `add_github_label`, `remove_github_label`, `close_github_issue`, `delete_github_issue`, `get_maintainer_issues`, `get_maintainer_issue_detail`, `get_worker_reports`

**Files:** Same three files, plus `src-tauri/src/commands/github.rs`.

**Key insight:** `commands/github.rs` already contains helper functions (`fetch_github_issues`, `extract_github_repo`, etc.) that are used by the Tauri commands. The server duplicates some of these helpers locally (e.g. `extract_github_repo_async`, `fetch_github_issues_async` in `server/main.rs`). Move the canonical implementations to `service.rs` and delete the server-side duplicates.

**Step 1-6:** Same TDD pattern.

```
refactor: extract GitHub issue commands into service layer
```

---

## Task 5: Migrate configuration, deploy, and voice commands

**Commands:**
- Config: `save_onboarding_config`, `home_dir`, `check_claude_cli`, `load_terminal_theme`, `load_keybindings`, `log_frontend_error`, `set_initial_prompt`
- Deploy: `detect_project_type`, `get_deploy_credentials`, `save_deploy_credentials`, `is_deploy_provisioned`, `deploy_project`, `list_deployed_services`
- Voice: `start_voice_pipeline`, `stop_voice_pipeline`, `toggle_voice_pause`
- Auth: `start_claude_login`, `stop_claude_login`

**Files:** Same three files, plus `src-tauri/src/deploy/commands.rs`.

**Key insight:** Most of these are trivial. Deploy commands already use the `deploy::` module. Voice commands interact with `VoicePipeline` on `AppState`. The service functions are thin wrappers.

**Step 1-6:** Same TDD pattern.

```
refactor: extract config, deploy, and voice commands into service layer
```

---

## Task 6: Migrate remaining commands

**Commands:**
- Directory listing: `list_directories_at`, `list_root_directories`
- Project names: `generate_project_names`
- Architecture: `generate_architecture`
- Git: `get_session_commits`, `get_repo_head`, `merge_session_branch`
- Prompts: `save_session_prompt`, `list_project_prompts`
- Token usage: `get_session_token_usage`
- Maintainer: `configure_maintainer`, `get_maintainer_status`, `get_maintainer_history`, `trigger_maintainer_check`, `clear_maintainer_reports`
- Auto-worker: `configure_auto_worker`, `get_auto_worker_queue`
- Secure env: `submit_secure_env_value`, `cancel_secure_env_request`
- Media stubs: `copy_image_file_to_clipboard`, `capture_app_screenshot`
- Scaffold: `scaffold_project`

**Key complexity:** `merge_session_branch` is complex (retries with rebase conflict resolution). `scaffold_project` is complex (git init, GitHub repo creation). Both exist with slightly different implementations in each mode — unify.

**Step 1-6:** Same TDD pattern.

```
refactor: extract remaining commands into service layer
```

---

## Task 7: Introduce typed request structs for server handlers

**Files:**
- Modify: `src-tauri/src/server/main.rs`
- Create: `src-tauri/src/server/requests.rs` (or inline in main.rs)

**Goal:** Replace all `Json<Value>` + manual `args["field"].as_str()` extraction with `#[derive(Deserialize)]` structs. This gives the server mode the same type-safe parameter validation that Tauri commands get for free.

**Step 1: Define request structs**

Group them by category. Example:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectSessionArgs {
    session_id: String,
    rows: Option<u16>,
    cols: Option<u16>,
}
```

**Step 2: Replace `Json<Value>` with `Json<XxxArgs>` in each handler**

This is mechanical — Axum's `Json` extractor automatically returns 422 for malformed requests, eliminating the manual `.ok_or_else(|| (BAD_REQUEST, ...))` boilerplate.

**Step 3: Test, lint, commit**

```
refactor: replace Json<Value> with typed request structs in server handlers
```

---

## Task 8: Split `service.rs` into submodules if needed

After all migrations, `service.rs` may be large. If it exceeds ~1500 lines, split into:

```
src/service/
  mod.rs          (re-exports)
  projects.rs     (project CRUD)
  sessions.rs     (session lifecycle, PTY ops)
  notes.rs        (notes CRUD, git commits)
  github.rs       (issue management)
  deploy.rs       (deployment ops)
  config.rs       (configuration, voice, auth)
```

This is purely organizational — no behavior change.

```
refactor: split service layer into submodules
```

---

## Task 9: Final cleanup and validation

**Step 1:** Run the full test suite:
```bash
cd src-tauri && cargo test
cd src-tauri && cargo test --features server
pnpm test
```

**Step 2:** Run all lint gates:
```bash
pnpm check
cd src-tauri && cargo fmt --check
cd src-tauri && cargo clippy -- -D warnings
cd src-tauri && cargo clippy --features server -- -D warnings
```

**Step 3:** Verify `server_routes_cover_desktop_command_surface` still passes — confirms no route was lost during migration.

**Step 4:** Delete any dead code in `commands.rs` (helper functions that moved to `service.rs`).

**Step 5:** Update `docs/domain-knowledge.md` with a note about the service layer pattern.

```
chore: final cleanup after service layer extraction
```

---

## Summary

| File | Before | After |
|---|---|---|
| `commands.rs` | ~3800 lines (logic + adapter) | ~600 lines (thin adapters only) |
| `server/main.rs` | ~3600 lines (logic + adapter) | ~1000 lines (routes + thin adapters) |
| `service.rs` (new) | — | ~2500 lines (single source of truth) |
| `error.rs` (new) | — | ~50 lines |

**Key invariant:** After each task, `cargo test` and `cargo test --features server` must pass. The migration is fully incremental — each task can be merged independently.

**Adding a new command after migration:**
1. Write the service function in `service.rs` (once)
2. Add a 3-line Tauri command adapter in `commands.rs`
3. Add a 5-line Axum handler adapter in `server/main.rs`
4. The `server_routes_cover_desktop_command_surface` test catches missing routes
