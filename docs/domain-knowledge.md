# Domain Knowledge

Lessons learned during development. Check this before making changes.

## Tauri v2: Synchronous Commands Block the Webview

**Problem:** Tauri commands defined as `pub fn` (synchronous) run on the **main thread**. If the command does anything slow (subprocess calls, file I/O, network), it freezes the entire webview — no rendering, no animations, no user interaction.

**Symptom:** UI appears "stuck" even though JavaScript has already updated the state. The browser can't paint because the main thread is blocked by the Rust command.

**Fix:** Make slow commands `pub async fn` and use `tokio::task::spawn_blocking` for CPU/IO-bound work:

```rust
// BAD: blocks main thread
#[tauri::command]
pub fn slow_command() -> Result<String, String> {
    let result = expensive_operation(); // freezes webview
    Ok(result)
}

// GOOD: runs on background thread
#[tauri::command]
pub async fn slow_command() -> Result<String, String> {
    let result = tokio::task::spawn_blocking(|| expensive_operation())
        .await
        .map_err(|e| format!("Task failed: {}", e))?;
    Ok(result)
}
```

**Rule of thumb:** Any command that shells out (`Command::new(...)`) or does significant I/O must be async + spawn_blocking.

## tmux Session Architecture

Sessions use tmux for process persistence. Two-layer PTY:

1. **tmux session** (`ctrl-{uuid}`): runs `claude` in a detached tmux session. Survives app exit.
2. **Attachment PTY**: local `portable-pty` running `tmux attach -t ctrl-{uuid}`. Reader thread reads from this. Dropped on app exit, re-created on restart.

Key behaviors:
- `spawn_session`: creates tmux session (if not exists) + attaches via local PTY
- `close_session`: kills tmux session + drops attachment PTY
- Intentional quit (`RunEvent::ExitRequested`): kills all tmux sessions
- Dev restart (process killed): no cleanup runs → tmux sessions survive → app reattaches on restart
- `CLAUDECODE` env var is removed on `tmux new-session`, not on `tmux attach`

tmux binary: resolved at runtime by checking `/opt/homebrew/bin/tmux`, then `/usr/local/bin/tmux`, then `tmux` on `PATH`. Session naming: `ctrl-{uuid}`.

Affected files:
- `src-tauri/src/tmux.rs` — tmux binary interactions
- `src-tauri/src/pty_manager.rs` — `spawn_session`, `close_session`, `attach_tmux_session`
- `src-tauri/src/lib.rs` — exit handler that kills tmux sessions

## Shell Environment Inheritance (macOS GUI)

macOS GUI apps inherit a minimal launchd environment missing `.zshrc` vars. `shell_env::inherit_shell_env()` resolves the user's full shell env at startup and applies it to the process. Must run before any threads (`set_var` is not thread-safe). For tmux, all process env vars are passed via `-e` flags in `build_create_args` because tmux sessions inherit the **server's** environment, not the client's.

Affected files: `src-tauri/src/shell_env.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/tmux.rs`

## CLAUDECODE Environment Variable

Claude Code sets a `CLAUDECODE` env var to detect nested sessions. All `Command::new("claude")` calls and PTY `CommandBuilder` spawns must include `.env_remove("CLAUDECODE")` to prevent "cannot be launched inside another Claude Code session" errors.

Affected locations:
- `src-tauri/src/tmux.rs` — `create_session` (removes CLAUDECODE for tmux-backed sessions)
- `src-tauri/src/pty_manager.rs` — `spawn_command` (removes CLAUDECODE for direct commands)
- `src-tauri/src/config.rs` — `check_claude_cli_status`, `generate_names_via_cli`
- `src-tauri/src/maintainer.rs` — `run_health_check` (removes CLAUDECODE for health check subprocess)

## Session Status Detection via Hooks

Session status (idle/working/exited) is detected using Claude Code hooks, not PTY output heuristics.

**How it works:**
1. On app startup, a Unix domain socket listener starts at `/tmp/the-controller.sock`.
2. When spawning Claude sessions, `--settings` is passed with hook config for `UserPromptSubmit` (→ working), `Stop` (→ idle), and `Notification[idle_prompt]` (→ idle).
3. Hook commands send `status:session-id` to the socket via `nc -U`.
4. The socket listener emits `session-status-hook:<session-id>` Tauri events.
5. PTY EOF (`session-status-changed`) still handles the "exited" state.

**Key files:**
- `src-tauri/src/status_socket.rs` — socket listener, message parsing, hook JSON generation
- `src-tauri/src/tmux.rs` — passes `--settings` and `THE_CONTROLLER_SESSION_ID` env var
- `src-tauri/src/pty_manager.rs` — same for direct (non-tmux) sessions
- `src/lib/Sidebar.svelte` — listens for `session-status-hook` events

**Edge cases:**
- Hook commands use `nc -w 2` + `; true` to avoid blocking Claude Code (`timeout` is not available on macOS)
- Stale socket files are cleaned up on startup
- Reattached tmux sessions default to "idle" until the next hook fires

## Server Mode (Headless Browser Deployment)

The Controller can run without a desktop environment as a standalone Axum HTTP/WebSocket server (`src-tauri/src/bin/server.rs`). The Vite-built frontend is served as static files and accessed via a web browser.

**How it works:**
- The `server` Cargo feature gates `axum` and `tower-http` dependencies. The server binary lives at `src/bin/server.rs` with `required-features = ["server"]`.
- `src/lib/backend.ts` checks for `__TAURI_INTERNALS__` at load time. In desktop mode, commands go through Tauri IPC; in browser mode, they become `POST /api/{command}` requests and events flow over a shared WebSocket at `/ws`.
- `src-tauri/src/emitter.rs` defines an `EventEmitter` trait with three implementations: `TauriEmitter` (desktop), `WsBroadcastEmitter` (server), and `NoopEmitter` (tests).
- `status_socket.rs` exposes `start_listener_with_state(Arc<AppState>)` so the server binary can receive Claude Code session hooks without a Tauri `AppHandle`.

**Auth:**
- Optional bearer token via `CONTROLLER_AUTH_TOKEN` env var.
- Token is passed as a URL query param (`?token=...`) on first load, moved to `sessionStorage`, and stripped from the URL to avoid leaking via history/referrer.
- WebSocket auth uses the same token as a query param.
- Auth middleware skips static file requests; only `/api/*` and `/ws` are gated.

**Desktop-only stubs:** `copy_image_file_to_clipboard`, `capture_app_screenshot`, `start_voice_pipeline`, `stop_voice_pipeline` return errors in server mode since they require native hardware access.

**Graceful shutdown:** The server handles `SIGTERM`/`SIGINT` by cleaning up the Unix domain socket and killing all PTY/tmux sessions.

**Key files:**
- `src-tauri/src/bin/server.rs` — Axum server (~2800 lines, 40+ routes)
- `src/lib/backend.ts` — dual-mode command/event routing
- `src/lib/platform.ts` — lazy Tauri imports with browser fallbacks
- `src-tauri/src/emitter.rs` — EventEmitter trait + implementations
- `src-tauri/src/status_socket.rs` — decoupled from AppHandle for server use
