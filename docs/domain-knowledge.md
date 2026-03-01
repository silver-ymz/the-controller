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

## CLAUDECODE Environment Variable

Claude Code sets a `CLAUDECODE` env var to detect nested sessions. All `Command::new("claude")` calls and PTY `CommandBuilder` spawns must include `.env_remove("CLAUDECODE")` to prevent "cannot be launched inside another Claude Code session" errors.

Affected locations:
- `src-tauri/src/pty_manager.rs` — `spawn_session`, `spawn_command`
- `src-tauri/src/config.rs` — `check_claude_cli_status`, `generate_names_via_cli`
