use std::sync::Arc;

use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::service;
use crate::state::AppState;

mod media;

/// Parse a UUID string, mapping failure to a `String` error (Tauri command
/// return type).
pub fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| e.to_string())
}

/// Run a blocking closure on a threadpool and propagate both join-failure and
/// `AppError` as `String` (Tauri command error type).
///
/// Usage:
/// ```ignore
/// let result = tauri_blocking!({
///     let foo = state.foo.clone();
///     move || service::do_thing(&foo, arg)
/// });
/// // Note: the macro already applies `?` internally; do not add `?` at the callsite.
/// ```
macro_rules! tauri_blocking {
    ($closure:expr) => {
        tauri::async_runtime::spawn_blocking($closure)
            .await
            .map_err(|e| format!("Task failed: {e}"))?
    };
}

// Re-export helpers that moved to the service layer so external callers
// (e.g. server/main.rs scaffold_project) keep working.
pub use crate::service::{ensure_claude_md_symlink, render_agents_md, validate_project_name};

// Re-export session helpers that moved to the service layer so external callers
// (e.g. auto_worker.rs, lib.rs, status_socket.rs, server/main.rs) keep working.
pub use crate::service::{kill_process_group, next_session_label, stage_session_core};

// Re-export git helpers that moved to the service layer.
pub use crate::service::{
    build_auto_worker_queue, find_main_branch_oid, validate_maintainer_interval,
};

// Re-export test-only helpers that moved to the service layer.
#[cfg(test)]
pub(crate) use crate::service::{
    cleanup_failed_session_spawn, find_staging_port, STAGING_PORT_OFFSET,
};

#[cfg(test)]
use crate::models::{CommitInfo, Project};

#[cfg(test)]
fn update_project_with_rollback<T, C, M, R, A>(
    state: &AppState,
    project_id: Uuid,
    mutate: M,
    rollback: R,
    action: A,
) -> Result<T, String>
where
    M: FnOnce(&mut Project) -> Result<C, String>,
    R: FnOnce(&mut Project) -> Result<(), String>,
    A: FnOnce(C) -> Result<T, String>,
{
    let action_context = {
        let storage = state.storage.lock().map_err(|e| e.to_string())?;
        let mut project = storage
            .load_project(project_id)
            .map_err(|e| e.to_string())?;
        let action_context = mutate(&mut project)?;
        storage.save_project(&project).map_err(|e| e.to_string())?;
        action_context
    };

    match action(action_context) {
        Ok(result) => Ok(result),
        Err(action_err) => {
            let rollback = (|| -> Result<(), String> {
                let storage = state.storage.lock().map_err(|e| e.to_string())?;
                let mut project = storage
                    .load_project(project_id)
                    .map_err(|e| e.to_string())?;
                rollback(&mut project)?;
                storage.save_project(&project).map_err(|e| e.to_string())
            })();

            match rollback {
                Ok(()) => Err(action_err),
                Err(rollback_err) => Err(format!(
                    "{} (rollback failed: {})",
                    action_err, rollback_err
                )),
            }
        }
    }
}

// Re-export wait_for_merge_rebase_resolution for tests
#[cfg(test)]
pub(crate) use crate::service::wait_for_merge_rebase_resolution;

/// Connect a terminal to its PTY session at the given size.
/// Called by each Terminal component after it measures its dimensions.
/// No-op if the session is already connected.
///
/// This command is async because it may talk to the PTY broker daemon,
/// which would block the main thread and prevent event delivery — including the
/// alternate-screen escape sequence that xterm.js needs for correct scrolling.
#[tauri::command]
pub async fn connect_session(
    state: State<'_, Arc<AppState>>,
    _app_handle: AppHandle,
    session_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    let id = parse_uuid(&session_id)?;

    let state = (*state).clone();

    tokio::task::spawn_blocking(move || {
        service::connect_session(&state, id, Some(rows), Some(cols)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| {
        tracing::error!(session_id = %id, error = %e, "failed to connect session");
        format!("Task failed: {}", e)
    })?
}

#[tauri::command]
pub async fn stage_session(
    state: State<'_, Arc<AppState>>,
    _app_handle: AppHandle,
    project_id: String,
    session_id: String,
) -> Result<(), String> {
    let project_uuid = parse_uuid(&project_id)?;
    let session_uuid = parse_uuid(&session_id)?;
    crate::service::stage_session_core(&state, project_uuid, session_uuid, true).await?;
    Ok(())
}

#[tauri::command]
pub async fn check_claude_cli() -> Result<String, String> {
    tauri_blocking!(|| crate::service::check_claude_cli().map_err(|e| e.to_string()))
}

#[tauri::command]
pub async fn copy_image_file_to_clipboard(app: AppHandle, path: String) -> Result<(), String> {
    media::copy_image_file_to_clipboard(app, path).await
}

#[tauri::command]
pub async fn capture_app_screenshot(app: AppHandle, cropped: bool) -> Result<String, String> {
    media::capture_app_screenshot(app, cropped).await
}

// Hand-written because image_bytes is Vec<u8> (binary data), which the
// derive_handlers macro cannot map correctly (it maps &[u8] → String).
#[tauri::command]
pub fn save_note_image(
    state: State<'_, Arc<AppState>>,
    folder: String,
    image_bytes: Vec<u8>,
    extension: String,
) -> Result<String, String> {
    service::save_note_image(&state, &folder, &image_bytes, &extension).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn merge_session_branch(
    state: State<'_, Arc<AppState>>,
    _app_handle: AppHandle,
    project_id: String,
    session_id: String,
) -> Result<crate::models::MergeResponse, String> {
    let project_uuid = parse_uuid(&project_id)?;
    let session_uuid = parse_uuid(&session_id)?;
    service::merge_session_branch(&state, project_uuid, session_uuid, true)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_maintainer_history(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Vec<crate::models::MaintainerRunLog>, String> {
    let project_uuid = parse_uuid(&project_id)?;
    service::get_maintainer_history(&state, project_uuid, 20).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn log_frontend_error(message: String, state: tauri::State<'_, Arc<AppState>>) {
    let _ = crate::service::log_frontend_error(&state, &message);
}

#[cfg(test)]
async fn run_maintainer_check_spawn_blocking_with<F>(
    repo_path: String,
    project_id: Uuid,
    github_repo: Option<String>,
    runner: F,
) -> Result<crate::models::MaintainerRunLog, String>
where
    F: FnOnce(String, Uuid, Option<String>) -> Result<crate::models::MaintainerRunLog, String>
        + Send
        + 'static,
{
    tauri_blocking!(move || runner(repo_path, project_id, github_repo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{save_config, Config};
    use crate::models::{MaintainerRunLog, SavedPrompt, SessionConfig};
    use crate::pty_manager::PtyManager;
    use crate::state::{AppState, IssueCache};
    use crate::storage::Storage;
    use crate::worktree::WorktreeManager;
    use once_cell::sync::Lazy;
    use std::env;
    use std::fs;
    use std::future::Future;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;
    use uuid::Uuid;

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
    const BUSY_TEST_WAIT: Duration = Duration::from_secs(5);

    fn make_test_state(base_dir: &Path, projects_root: &Path) -> Arc<AppState> {
        let storage = Storage::new(base_dir.to_path_buf());
        storage.ensure_dirs().expect("ensure_dirs");
        save_config(
            base_dir,
            &Config {
                projects_root: projects_root.to_string_lossy().to_string(),
                default_provider: crate::config::ConfigDefaultProvider::ClaudeCode,
                log_level: "info".to_string(),
            },
        )
        .expect("save_config");

        Arc::new(AppState {
            storage: Arc::new(Mutex::new(storage)),
            pty_manager: Arc::new(Mutex::new(PtyManager::new())),
            issue_cache: Arc::new(Mutex::new(IssueCache::new())),
            secure_env_request: Mutex::new(None),
            emitter: crate::emitter::NoopEmitter::new(),
            staging_lock: tokio::sync::Mutex::new(()),
            voice_pipeline: Arc::new(tokio::sync::Mutex::new(None)),
            frontend_log: Mutex::new(None),
            voice_generation: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn run_async_test<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime")
            .block_on(future)
    }

    fn real_git_path() -> String {
        let output = std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("locate git");
        assert!(output.status.success(), "which git should succeed");
        String::from_utf8(output.stdout)
            .expect("utf8 git path")
            .trim()
            .to_string()
    }

    fn write_fake_command(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{}\n", body)).expect("write fake command");
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(path).expect("stat fake command").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).expect("chmod fake command");
        }
    }

    fn with_fake_cli_bins<T>(f: impl FnOnce(&Path, &Path, &Path) -> T) -> T {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let bin_dir = TempDir::new().expect("temp fake cli dir");
        let state_dir = TempDir::new().expect("temp scaffold state dir");
        let gh_path = bin_dir.path().join("gh");
        let git_path = bin_dir.path().join("git");
        let original_gh = env::var_os("THE_CONTROLLER_GH_BIN");
        let original_git = env::var_os("THE_CONTROLLER_GIT_BIN");

        env::set_var("THE_CONTROLLER_GH_BIN", &gh_path);
        env::set_var("THE_CONTROLLER_GIT_BIN", &git_path);

        let result = f(&gh_path, &git_path, state_dir.path());

        match original_gh {
            Some(path) => env::set_var("THE_CONTROLLER_GH_BIN", path),
            None => env::remove_var("THE_CONTROLLER_GH_BIN"),
        }
        match original_git {
            Some(path) => env::set_var("THE_CONTROLLER_GIT_BIN", path),
            None => env::remove_var("THE_CONTROLLER_GIT_BIN"),
        }

        result
    }

    fn wait_for_path(path: &Path, timeout: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if path.exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        path.exists()
    }

    #[test]
    fn test_wait_for_merge_rebase_resolution_times_out() {
        run_async_test(async {
            let checks = Arc::new(AtomicUsize::new(0));
            let checks_for_closure = Arc::clone(&checks);

            let result = wait_for_merge_rebase_resolution(
                move || {
                    let checks_for_closure = Arc::clone(&checks_for_closure);
                    async move {
                        checks_for_closure.fetch_add(1, Ordering::SeqCst);
                        Ok(true)
                    }
                },
                2,
                Duration::from_millis(1),
            )
            .await;
            assert_eq!(
                result.expect_err("wait should time out"),
                "Timed out waiting for merge conflict resolution."
            );
            assert_eq!(checks.load(Ordering::SeqCst), 2);
        });
    }

    #[test]
    fn test_load_terminal_theme_returns_default_when_theme_file_is_missing() {
        let tmp = TempDir::new().expect("temp dir");
        let projects_root = tmp.path().join("projects-root");
        fs::create_dir_all(&projects_root).expect("create projects root");
        let state = make_test_state(tmp.path(), &projects_root);

        let theme =
            service::load_terminal_theme_blocking(&state).expect("theme command should succeed");

        assert_eq!(theme.background, "#000000");
        assert_eq!(theme.foreground, "#e0e0e0");
        assert_eq!(theme.cursor, "#ffffff");
        assert_eq!(theme.selection_background, "#2e2e2e");
    }

    #[test]
    fn test_load_terminal_theme_reads_kitty_style_theme_file_from_base_dir() {
        let tmp = TempDir::new().expect("temp dir");
        let projects_root = tmp.path().join("projects-root");
        fs::create_dir_all(&projects_root).expect("create projects root");
        fs::write(
            tmp.path().join("current-theme.conf"),
            "\
background #121212
foreground #f0f0f0
cursor #ff9900
selection_background #444444
",
        )
        .expect("write theme file");
        let state = make_test_state(tmp.path(), &projects_root);

        let theme =
            service::load_terminal_theme_blocking(&state).expect("theme command should succeed");

        assert_eq!(theme.background, "#121212");
        assert_eq!(theme.foreground, "#f0f0f0");
        assert_eq!(theme.cursor, "#ff9900");
        assert_eq!(theme.selection_background, "#444444");
    }

    #[test]
    fn test_wait_for_merge_rebase_resolution_stops_once_rebase_clears() {
        run_async_test(async {
            let checks = Arc::new(AtomicUsize::new(0));
            let checks_for_closure = Arc::clone(&checks);

            let result = wait_for_merge_rebase_resolution(
                move || {
                    let checks_for_closure = Arc::clone(&checks_for_closure);
                    async move {
                        let attempt = checks_for_closure.fetch_add(1, Ordering::SeqCst);
                        Ok(attempt == 0)
                    }
                },
                5,
                Duration::from_millis(1),
            )
            .await;

            assert!(result.is_ok(), "wait should stop after rebase clears");
            assert_eq!(checks.load(Ordering::SeqCst), 2);
        });
    }

    // --- validate_project_name tests ---

    #[test]
    fn test_valid_project_name() {
        assert!(validate_project_name("my-cool-project").is_ok());
    }

    #[test]
    fn test_empty_project_name() {
        let result = validate_project_name("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid project name"));
    }

    #[test]
    fn test_project_name_with_forward_slash() {
        let result = validate_project_name("foo/bar");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid project name"));
    }

    #[test]
    fn test_project_name_with_backslash() {
        let result = validate_project_name("foo\\bar");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid project name"));
    }

    #[test]
    fn test_project_name_starting_with_dot() {
        let result = validate_project_name(".hidden");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid project name"));
    }

    // --- next_session_label tests ---

    #[test]
    fn test_next_session_label_empty() {
        let sessions: Vec<SessionConfig> = vec![];
        let label = next_session_label(&sessions);
        assert!(
            label.starts_with("session-1-"),
            "expected session-1-<uuid>, got {}",
            label
        );
        assert_eq!(label.len(), "session-1-".len() + 6);
    }

    #[test]
    fn test_next_session_label_two_existing() {
        let sessions = vec![
            SessionConfig {
                id: Uuid::new_v4(),
                label: "session-1".to_string(),
                worktree_path: None,
                worktree_branch: None,
                archived: false,
                kind: "claude".to_string(),
                github_issue: None,
                initial_prompt: None,
                done_commits: vec![],
                auto_worker_session: false,
            },
            SessionConfig {
                id: Uuid::new_v4(),
                label: "session-2".to_string(),
                worktree_path: None,
                worktree_branch: None,
                archived: false,
                kind: "claude".to_string(),
                github_issue: None,
                initial_prompt: None,
                done_commits: vec![],
                auto_worker_session: false,
            },
        ];
        let label = next_session_label(&sessions);
        assert!(
            label.starts_with("session-3-"),
            "expected session-3-<uuid>, got {}",
            label
        );
    }

    #[test]
    fn test_next_session_label_counts_all_sessions() {
        // Archived sessions still occupy worktree branches, so they must be counted
        let sessions = vec![
            SessionConfig {
                id: Uuid::new_v4(),
                label: "session-1".to_string(),
                worktree_path: Some("/tmp/wt1".to_string()),
                worktree_branch: Some("session-1".to_string()),
                archived: true,
                kind: "claude".to_string(),
                github_issue: None,
                initial_prompt: None,
                done_commits: vec![],
                auto_worker_session: false,
            },
            SessionConfig {
                id: Uuid::new_v4(),
                label: "session-2".to_string(),
                worktree_path: Some("/tmp/wt2".to_string()),
                worktree_branch: Some("session-2".to_string()),
                archived: false,
                kind: "claude".to_string(),
                github_issue: None,
                initial_prompt: None,
                done_commits: vec![],
                auto_worker_session: false,
            },
            SessionConfig {
                id: Uuid::new_v4(),
                label: "session-3".to_string(),
                worktree_path: Some("/tmp/wt3".to_string()),
                worktree_branch: Some("session-3".to_string()),
                archived: true,
                kind: "claude".to_string(),
                github_issue: None,
                initial_prompt: None,
                done_commits: vec![],
                auto_worker_session: false,
            },
        ];
        // Max is session-3, so next is session-4
        let label = next_session_label(&sessions);
        assert!(
            label.starts_with("session-4-"),
            "expected session-4-<uuid>, got {}",
            label
        );
    }

    #[test]
    fn test_next_session_label_with_gap() {
        // Only session-3 remains (1 and 2 deleted). Next should be session-4, not session-2.
        let sessions = vec![SessionConfig {
            id: Uuid::new_v4(),
            label: "session-3".to_string(),
            worktree_path: None,
            worktree_branch: None,
            archived: false,
            kind: "claude".to_string(),
            github_issue: None,
            initial_prompt: None,
            done_commits: vec![],
            auto_worker_session: false,
        }];
        let label = next_session_label(&sessions);
        assert!(
            label.starts_with("session-4-"),
            "expected session-4-<uuid>, got {}",
            label
        );
    }

    // --- render_agents_md tests ---

    #[test]
    fn test_render_agents_md_replaces_name() {
        let result = render_agents_md("my-project");
        assert!(result.starts_with("# my-project"));
        assert!(!result.contains("{name}"));
    }

    #[test]
    fn test_render_agents_md_empty_name() {
        let result = render_agents_md("");
        assert!(result.starts_with("# \n"));
    }

    #[test]
    fn test_scaffold_project_rolls_back_directory_when_github_creation_fails() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let repo_path = projects_root.path().join("gh-create-failure-test");
        let real_git = real_git_path();

        with_fake_cli_bins(|gh_path, git_path, state_dir| {
            let state_dir_display = state_dir.display().to_string();
            write_fake_command(
                gh_path,
                &format!(
                    "if [ -f \"{state_dir_display}/gh-create-fails\" ]; then\n  echo \"gh create failed\" >&2\n  exit 1\nfi\nif [ \"$1\" = \"repo\" ] && [ \"$2\" = \"create\" ]; then\n  \"{real_git}\" -C \"$PWD\" remote add origin git@github.com:test-owner/gh-create-failure-test.git\n  touch \"{state_dir_display}/remote-created\"\n  exit 0\nfi\necho \"unexpected gh invocation: $*\" >&2\nexit 1"
                ),
            );
            write_fake_command(
                git_path,
                "if [ \"$1\" = \"push\" ]; then\n  exit 0\nfi\necho \"unexpected git invocation: $*\" >&2\nexit 1",
            );
            fs::write(state_dir.join("gh-create-fails"), "")
                .expect("mark first gh create as failed");

            let error = run_async_test(service::scaffold_project(
                &app_state,
                "gh-create-failure-test",
            ))
            .map_err(|e| e.to_string())
            .expect_err("gh create failure should bubble up");
            assert!(error.contains("Failed to create GitHub repo"));
            assert!(
                !repo_path.exists(),
                "repo directory should be removed after gh repo create failure"
            );
            assert!(
                !state_dir.join("remote-created").exists(),
                "gh create failure should not leave remote state behind"
            );

            let stored = app_state
                .storage
                .lock()
                .unwrap()
                .list_projects()
                .expect("list projects after failed scaffold");
            assert!(
                stored.is_empty(),
                "failed scaffold should not persist project state"
            );

            fs::remove_file(state_dir.join("gh-create-fails")).expect("allow gh create retry");

            let project = run_async_test(service::scaffold_project(
                &app_state,
                "gh-create-failure-test",
            ))
            .map_err(|e| e.to_string())
            .expect("retry should succeed after rollback");
            assert_eq!(project.name, "gh-create-failure-test");
            assert!(repo_path.exists(), "retry should recreate repo directory");
            assert!(
                state_dir.join("remote-created").exists(),
                "successful retry should create the remote"
            );
        });
    }

    #[test]
    fn test_scaffold_project_does_not_hold_storage_lock_during_external_publish() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let real_git = real_git_path();

        with_fake_cli_bins(|gh_path, git_path, state_dir| {
            let state_dir_display = state_dir.display().to_string();
            write_fake_command(
                gh_path,
                &format!(
                    "if [ \"$1\" = \"repo\" ] && [ \"$2\" = \"create\" ]; then\n  touch \"{state_dir_display}/gh-create-started\"\n  while [ -f \"{state_dir_display}/gh-create-block\" ]; do\n    sleep 0.05\n  done\n  \"{real_git}\" -C \"$PWD\" remote add origin git@github.com:test-owner/lock-scope-test.git\n  exit 0\nfi\necho \"unexpected gh invocation: $*\" >&2\nexit 1"
                ),
            );
            write_fake_command(git_path, "if [ \"$1\" = \"push\" ]; then\n  exit 0\nfi\necho \"unexpected git invocation: $*\" >&2\nexit 1");
            fs::write(state_dir.join("gh-create-block"), "").expect("block gh create");

            let app_state_for_thread = Arc::clone(&app_state);
            let handle = thread::spawn(move || {
                run_async_test(service::scaffold_project(
                    &app_state_for_thread,
                    "lock-scope-test",
                ))
                .map_err(|e| e.to_string())
            });

            assert!(
                wait_for_path(&state_dir.join("gh-create-started"), BUSY_TEST_WAIT),
                "scaffold should reach gh repo create"
            );

            let storage_lock_available = app_state.storage.try_lock().is_ok();

            fs::remove_file(state_dir.join("gh-create-block")).expect("unblock gh create");

            let project = handle
                .join()
                .expect("scaffold thread should not panic")
                .expect("scaffold should succeed after gh unblock");

            assert_eq!(project.name, "lock-scope-test");
            assert!(
                storage_lock_available,
                "storage lock should stay available while external publish is blocked"
            );
        });
    }

    #[test]
    fn test_scaffold_project_rolls_back_if_name_is_claimed_before_final_save() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let repo_path = projects_root.path().join("lock-race-test");
        let imported_repo = TempDir::new().unwrap();
        let real_git = real_git_path();

        with_fake_cli_bins(|gh_path, git_path, state_dir| {
            let state_dir_display = state_dir.display().to_string();
            write_fake_command(
                gh_path,
                &format!(
                    "if [ \"$1\" = \"repo\" ] && [ \"$2\" = \"create\" ]; then\n  touch \"{state_dir_display}/gh-create-started\"\n  while [ -f \"{state_dir_display}/gh-create-block\" ]; do\n    sleep 0.05\n  done\n  \"{real_git}\" -C \"$PWD\" remote add origin git@github.com:test-owner/lock-race-test.git\n  touch \"{state_dir_display}/remote-created\"\n  exit 0\nfi\nif [ \"$1\" = \"repo\" ] && [ \"$2\" = \"delete\" ]; then\n  rm -f \"{state_dir_display}/remote-created\"\n  touch \"{state_dir_display}/remote-deleted\"\n  exit 0\nfi\necho \"unexpected gh invocation: $*\" >&2\nexit 1"
                ),
            );
            write_fake_command(git_path, "if [ \"$1\" = \"push\" ]; then\n  exit 0\nfi\necho \"unexpected git invocation: $*\" >&2\nexit 1");
            fs::write(state_dir.join("gh-create-block"), "").expect("block gh create");

            let app_state_for_thread = Arc::clone(&app_state);
            let handle = thread::spawn(move || {
                run_async_test(service::scaffold_project(
                    &app_state_for_thread,
                    "lock-race-test",
                ))
                .map_err(|e| e.to_string())
            });

            assert!(
                wait_for_path(&state_dir.join("gh-create-started"), BUSY_TEST_WAIT),
                "scaffold should reach gh repo create"
            );

            let imported = service::create_project(
                &app_state,
                "lock-race-test",
                &imported_repo.path().to_string_lossy(),
            )
            .map_err(|e| e.to_string())
            .expect("concurrent create_project should claim the name");
            assert_eq!(imported.name, "lock-race-test");

            fs::remove_file(state_dir.join("gh-create-block")).expect("unblock gh create");

            let error = handle
                .join()
                .expect("scaffold thread should not panic")
                .expect_err("scaffold should fail once the name is claimed");

            assert!(
                error.contains("A project named 'lock-race-test' already exists"),
                "expected duplicate-name failure, got: {error}"
            );
            assert!(
                !repo_path.exists(),
                "scaffold repo should be rolled back if final save loses the name race"
            );
            assert!(
                state_dir.join("remote-deleted").exists(),
                "remote repo should be deleted when the final save loses the name race"
            );

            let stored = app_state
                .storage
                .lock()
                .unwrap()
                .list_projects()
                .expect("list projects after duplicate-name race");
            assert_eq!(
                stored.projects.len(),
                1,
                "only the competing project should remain after rollback"
            );
            assert_eq!(
                stored.projects[0].repo_path,
                imported_repo.path().to_string_lossy()
            );
        });
    }

    // --- find_main_branch_oid tests ---

    #[test]
    fn test_find_main_branch_oid_with_main_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = git2::Repository::init(tmp.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        // After init + commit, HEAD points to a branch — find_main_branch_oid should find it
        let oid = find_main_branch_oid(&repo);
        assert!(oid.is_some());
    }

    #[test]
    fn test_find_main_branch_oid_no_main_or_master() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = git2::Repository::init(tmp.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        // Rename the default branch to something other than main/master
        let commit = repo.find_commit(oid).unwrap();
        repo.branch("develop", &commit, false).unwrap();
        // Delete the original branch by setting HEAD to develop
        repo.set_head("refs/heads/develop").unwrap();
        if let Ok(mut branch) = repo.find_branch("main", git2::BranchType::Local) {
            let _ = branch.delete();
        }
        if let Ok(mut branch) = repo.find_branch("master", git2::BranchType::Local) {
            let _ = branch.delete();
        }

        let result = find_main_branch_oid(&repo);
        assert!(result.is_none());
    }

    // --- CommitInfo serialization ---

    #[test]
    fn test_commit_info_serialization() {
        let info = CommitInfo {
            hash: "abc1234".to_string(),
            message: "fix: resolve bug".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["hash"], "abc1234");
        assert_eq!(parsed["message"], "fix: resolve bug");
    }

    // --- additional validate_project_name edge cases ---

    #[test]
    fn test_project_name_with_spaces_is_valid() {
        // Spaces are not explicitly rejected
        assert!(validate_project_name("my project").is_ok());
    }

    #[test]
    fn test_project_name_with_hyphens_and_underscores() {
        assert!(validate_project_name("my-cool_project-2").is_ok());
    }

    #[test]
    fn test_project_name_single_char() {
        assert!(validate_project_name("a").is_ok());
    }

    #[test]
    fn test_scaffold_project_rolls_back_remote_and_local_state_when_initial_push_fails() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let repo_path = projects_root.path().join("rollback-test");
        let real_git = real_git_path();

        with_fake_cli_bins(|gh_path, git_path, state_dir| {
            let state_dir_display = state_dir.display().to_string();
            write_fake_command(
                gh_path,
                &format!(
                    "if [ \"$1\" = \"repo\" ] && [ \"$2\" = \"create\" ]; then\n  \"{real_git}\" -C \"$PWD\" remote add origin git@github.com:test-owner/rollback-test.git\n  touch \"{state_dir_display}/remote-created\"\n  exit 0\nfi\nif [ \"$1\" = \"repo\" ] && [ \"$2\" = \"delete\" ]; then\n  if [ \"$3\" != \"test-owner/rollback-test\" ]; then\n    echo \"unexpected repo delete target: $3\" >&2\n    exit 1\n  fi\n  rm -f \"{state_dir_display}/remote-created\"\n  touch \"{state_dir_display}/remote-deleted\"\n  exit 0\nfi\necho \"unexpected gh invocation: $*\" >&2\nexit 1"
                ),
            );
            write_fake_command(
                git_path,
                &format!(
                    "if [ -f \"{state_dir_display}/push-fails\" ]; then\n  echo \"push failed\" >&2\n  exit 1\nfi\nexit 0"
                ),
            );
            fs::write(state_dir.join("push-fails"), "").expect("mark first push as failed");

            let error = run_async_test(service::scaffold_project(&app_state, "rollback-test"))
                .map_err(|e| e.to_string())
                .expect_err("push failure should bubble up");
            assert!(error.contains("Failed to push initial commit"));
            assert!(
                !repo_path.exists(),
                "repo directory should be removed after push failure"
            );
            assert!(
                state_dir.join("remote-deleted").exists(),
                "remote repo created during scaffold should be deleted on push failure"
            );

            let stored = app_state
                .storage
                .lock()
                .unwrap()
                .list_projects()
                .expect("list projects after failed scaffold");
            assert!(
                stored.projects.is_empty(),
                "failed scaffold should not persist project state"
            );

            fs::remove_file(state_dir.join("push-fails")).expect("allow retry push");
            fs::remove_file(state_dir.join("remote-deleted"))
                .expect("clear previous delete marker");

            let project = run_async_test(service::scaffold_project(&app_state, "rollback-test"))
                .map_err(|e| e.to_string())
                .expect("retry should succeed after rollback");
            assert_eq!(project.name, "rollback-test");
            assert!(repo_path.exists(), "retry should recreate repo directory");
            assert!(
                state_dir.join("remote-created").exists(),
                "successful retry should recreate the remote"
            );

            let stored = app_state
                .storage
                .lock()
                .unwrap()
                .list_projects()
                .expect("list projects after successful retry");
            assert_eq!(
                stored.projects.len(),
                1,
                "successful retry should persist exactly one project"
            );
            assert_eq!(stored.projects[0].repo_path, repo_path.to_string_lossy());
        });
    }

    #[test]
    fn test_rollback_session_metadata_for_create_session_path() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());

        let project = service::create_project(
            &app_state,
            "rollback-session-create",
            &repo_dir.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect("create project");

        let session_id = Uuid::new_v4();
        let error = update_project_with_rollback(
            &app_state,
            project.id,
            |project| {
                project.sessions.push(SessionConfig {
                    id: session_id,
                    label: "session-1".to_string(),
                    worktree_path: Some("/tmp/worktree".to_string()),
                    worktree_branch: Some("session-1".to_string()),
                    archived: false,
                    kind: "claude".to_string(),
                    github_issue: None,
                    initial_prompt: None,
                    done_commits: vec![],
                    auto_worker_session: false,
                });
                Ok(())
            },
            |project| {
                project.sessions.retain(|session| session.id != session_id);
                Ok(())
            },
            |()| Err::<(), String>("spawn failed".to_string()),
        )
        .expect_err("post-save failure should bubble up");

        assert_eq!(error, "spawn failed");

        let stored = app_state
            .storage
            .lock()
            .unwrap()
            .load_project(project.id)
            .expect("load project after rollback");
        assert!(
            stored.sessions.is_empty(),
            "failed create-session path should remove persisted session metadata"
        );
    }

    #[test]
    fn test_rollback_session_metadata_preserves_concurrent_project_updates() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());

        let project = service::create_project(
            &app_state,
            "rollback-session-concurrency",
            &repo_dir.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect("create project");

        let session_id = Uuid::new_v4();
        let prompt_id = Uuid::new_v4();
        let error = update_project_with_rollback(
            &app_state,
            project.id,
            |project| {
                project.sessions.push(SessionConfig {
                    id: session_id,
                    label: "session-1".to_string(),
                    worktree_path: Some("/tmp/worktree".to_string()),
                    worktree_branch: Some("session-1".to_string()),
                    archived: false,
                    kind: "claude".to_string(),
                    github_issue: None,
                    initial_prompt: None,
                    done_commits: vec![],
                    auto_worker_session: false,
                });
                Ok(())
            },
            |project| {
                project.sessions.retain(|session| session.id != session_id);
                Ok(())
            },
            |()| {
                let storage = app_state.storage.lock().unwrap();
                let mut latest = storage
                    .load_project(project.id)
                    .expect("load latest project");
                latest.prompts.push(SavedPrompt {
                    id: prompt_id,
                    name: "Concurrent prompt".to_string(),
                    text: "Preserve me".to_string(),
                    created_at: "2026-03-10T00:00:00Z".to_string(),
                    source_session_label: "session-elsewhere".to_string(),
                });
                storage
                    .save_project(&latest)
                    .expect("save concurrent update");
                Err::<(), String>("spawn failed".to_string())
            },
        )
        .expect_err("post-save failure should bubble up");

        assert_eq!(error, "spawn failed");

        let stored = app_state
            .storage
            .lock()
            .unwrap()
            .load_project(project.id)
            .expect("load project after rollback");
        assert!(
            stored.sessions.is_empty(),
            "rollback should still remove the failed session metadata"
        );
        assert_eq!(stored.prompts.len(), 1);
        assert_eq!(stored.prompts[0].id, prompt_id);
    }

    #[test]
    fn test_cleanup_failed_session_spawn_removes_created_worktree() {
        let repo_dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(repo_dir.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        let worktree_root = TempDir::new().unwrap();
        let branch = "session-cleanup";
        let worktree_path = WorktreeManager::create_worktree(
            &repo_dir.path().to_string_lossy(),
            branch,
            &worktree_root.path().join(branch),
        )
        .expect("create worktree");

        cleanup_failed_session_spawn(
            &repo_dir.path().to_string_lossy(),
            Some(worktree_path.to_str().unwrap()),
            Some(branch),
        )
        .expect("cleanup created worktree");

        assert!(
            !worktree_path.exists(),
            "failed session spawn cleanup should remove the worktree directory"
        );
        assert!(
            repo.find_branch(branch, git2::BranchType::Local).is_err(),
            "failed session spawn cleanup should remove the branch reference"
        );
    }

    #[test]
    fn test_create_project_succeeds_with_corrupt_sibling_metadata() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let repo_dir = TempDir::new().unwrap();

        let corrupt_dir = base_dir
            .path()
            .join("projects")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&corrupt_dir).expect("create corrupt dir");
        fs::write(corrupt_dir.join("project.json"), "{ invalid json").expect("write corrupt json");

        let project = service::create_project(
            &app_state,
            "fresh-project",
            &repo_dir.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect("create project should ignore corrupt sibling metadata");

        assert_eq!(project.name, "fresh-project");
    }

    #[test]
    fn test_create_project_rejects_invalid_project_name() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let repo_dir = TempDir::new().unwrap();

        let error = service::create_project(
            &app_state,
            "invalid/name",
            &repo_dir.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect_err("create_project should reject invalid names");

        assert!(error.contains("Invalid project name: invalid/name"));
    }

    #[test]
    fn test_create_project_rejects_duplicate_name_even_if_existing_project_is_archived() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let existing_repo = TempDir::new().unwrap();
        let new_repo = TempDir::new().unwrap();

        {
            let storage = app_state.storage.lock().expect("lock storage");
            storage
                .save_project(&Project {
                    id: Uuid::new_v4(),
                    name: "duplicate-name".to_string(),
                    repo_path: existing_repo.path().to_string_lossy().to_string(),
                    created_at: "2026-03-10T00:00:00Z".to_string(),
                    archived: true,
                    maintainer: crate::models::MaintainerConfig::default(),
                    auto_worker: crate::models::AutoWorkerConfig::default(),
                    prompts: vec![],
                    sessions: vec![],
                    staged_sessions: vec![],
                })
                .expect("save existing project");
        }

        let error = service::create_project(
            &app_state,
            "duplicate-name",
            &new_repo.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect_err("create_project should reject duplicate names regardless of archived flag");

        assert_eq!(error, "A project named 'duplicate-name' already exists");
    }

    #[test]
    fn test_load_project_succeeds_with_corrupt_sibling_metadata() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let repo_dir = TempDir::new().unwrap();
        git2::Repository::init(repo_dir.path()).expect("init git repo");

        let corrupt_dir = base_dir
            .path()
            .join("projects")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&corrupt_dir).expect("create corrupt dir");
        fs::write(corrupt_dir.join("project.json"), "{ invalid json").expect("write corrupt json");

        let project = service::load_project(
            &app_state,
            "imported-project",
            &repo_dir.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect("load project should ignore corrupt sibling metadata");

        assert_eq!(project.name, "imported-project");
    }

    #[test]
    fn test_load_project_rejects_invalid_project_name() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let repo_dir = TempDir::new().unwrap();
        git2::Repository::init(repo_dir.path()).expect("init git repo");

        let error = service::load_project(
            &app_state,
            "invalid/name",
            &repo_dir.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect_err("load_project should reject invalid names");

        assert!(error.contains("Invalid project name: invalid/name"));
    }

    #[test]
    fn test_list_projects_includes_projects_marked_archived_in_storage() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());

        {
            let storage = app_state.storage.lock().expect("lock storage");
            storage
                .save_project(&Project {
                    id: Uuid::new_v4(),
                    name: "stored-project".to_string(),
                    repo_path: projects_root
                        .path()
                        .join("stored-project")
                        .to_string_lossy()
                        .to_string(),
                    created_at: "2026-03-10T00:00:00Z".to_string(),
                    archived: true,
                    maintainer: crate::models::MaintainerConfig::default(),
                    auto_worker: crate::models::AutoWorkerConfig::default(),
                    prompts: vec![],
                    sessions: vec![],
                    staged_sessions: vec![],
                })
                .expect("save archived-flagged project");
        }

        let inventory = service::list_projects(&app_state)
            .map_err(|e| e.to_string())
            .expect("list projects");

        assert_eq!(inventory.projects.len(), 1);
        assert_eq!(inventory.projects[0].name, "stored-project");
    }

    #[test]
    fn test_scaffold_project_succeeds_with_corrupt_sibling_metadata() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let repo_path = projects_root.path().join("scaffold-with-corrupt-sibling");
        let real_git = real_git_path();

        let corrupt_dir = base_dir
            .path()
            .join("projects")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&corrupt_dir).expect("create corrupt dir");
        fs::write(corrupt_dir.join("project.json"), "{ invalid json").expect("write corrupt json");

        with_fake_cli_bins(|gh_path, git_path, _state_dir| {
            write_fake_command(
                gh_path,
                &format!(
                    "if [ \"$1\" = \"repo\" ] && [ \"$2\" = \"create\" ]; then\n  \"{real_git}\" -C \"$PWD\" remote add origin git@github.com:test-owner/scaffold-with-corrupt-sibling.git\n  exit 0\nfi\necho \"unexpected gh invocation: $*\" >&2\nexit 1"
                ),
            );
            write_fake_command(git_path, "exit 0");

            let project = run_async_test(service::scaffold_project(
                &app_state,
                "scaffold-with-corrupt-sibling",
            ))
            .map_err(|e| e.to_string())
            .expect("scaffold should ignore corrupt sibling metadata");

            assert_eq!(project.name, "scaffold-with-corrupt-sibling");
            assert!(repo_path.exists(), "repo should be created");
        });
    }

    // --- validate_maintainer_interval tests ---

    #[test]
    fn test_validate_interval_minutes() {
        assert!(validate_maintainer_interval(5).is_ok());
        assert!(validate_maintainer_interval(60).is_ok());
        assert!(validate_maintainer_interval(1440).is_ok());
        assert!(validate_maintainer_interval(0).is_err());
        assert!(validate_maintainer_interval(4).is_err());
    }

    #[test]
    fn test_trigger_maintainer_check_uses_spawn_blocking() {
        // The blocking work now lives in service::trigger_maintainer_check
        let source = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service/maintainer.rs"),
        )
        .expect("read service source");
        let start = source
            .find("pub async fn trigger_maintainer_check")
            .expect("find trigger_maintainer_check");
        let rest = &source[start..];
        let end = rest
            .find("\npub ")
            .or_else(|| rest.find("\n#[cfg(test)]"))
            .expect("find end of trigger_maintainer_check");
        let function_body = &rest[..end];

        assert!(
            function_body.contains("spawn_blocking"),
            "trigger_maintainer_check must offload blocking maintainer work with spawn_blocking"
        );
    }

    #[test]
    fn test_run_maintainer_check_spawn_blocking_with_offloads_work() {
        run_async_test(async {
            let runtime_thread_id = thread::current().id();
            let project_id = Uuid::new_v4();

            let log = run_maintainer_check_spawn_blocking_with(
                "/tmp/project".to_string(),
                project_id,
                Some("owner/repo".to_string()),
                move |repo_path, inner_project_id, github_repo| {
                    assert_eq!(repo_path, "/tmp/project");
                    assert_eq!(inner_project_id, project_id);
                    assert_eq!(github_repo.as_deref(), Some("owner/repo"));
                    assert_ne!(thread::current().id(), runtime_thread_id);

                    Ok(MaintainerRunLog {
                        id: Uuid::new_v4(),
                        project_id: inner_project_id,
                        timestamp: "2026-03-10T00:00:00Z".to_string(),
                        issues_filed: vec![],
                        issues_updated: vec![],
                        issues_unchanged: 0,
                        issues_skipped: 0,
                        summary: "No actionable maintainer issues found".to_string(),
                    })
                },
            )
            .await
            .expect("maintainer check should succeed");

            assert_eq!(log.project_id, project_id);
        });
    }

    fn make_issue(number: u64, title: &str, labels: &[&str]) -> crate::models::GithubIssue {
        crate::models::GithubIssue {
            number,
            title: title.to_string(),
            url: format!("https://github.com/example/repo/issues/{number}"),
            body: Some(format!("Body for issue {number}")),
            labels: labels
                .iter()
                .map(|label| crate::models::GithubLabel {
                    name: (*label).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn test_build_auto_worker_queue_filters_and_pins_active_issue() {
        let issues = vec![
            make_issue(
                11,
                "Eligible queued issue",
                &["priority:high", "complexity:low"],
            ),
            make_issue(
                12,
                "Busy issue",
                &["priority:high", "complexity:low", "in-progress"],
            ),
        ];
        let active_issue = Some(make_issue(
            12,
            "Busy issue",
            &[
                "priority:high",
                "complexity:low",
                "in-progress",
                "assigned-to-auto-worker",
            ],
        ));

        let queue = build_auto_worker_queue(issues, active_issue);

        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].number, 12);
        assert!(queue[0].is_active);
        assert_eq!(queue[1].number, 11);
        assert!(!queue[1].is_active);
    }

    #[test]
    fn test_build_auto_worker_queue_avoids_duplicate_active_issue() {
        let issues = vec![make_issue(
            21,
            "Already active",
            &["priority:high", "complexity:low"],
        )];
        let active_issue = Some(make_issue(
            21,
            "Already active",
            &["priority:high", "complexity:low"],
        ));

        let queue = build_auto_worker_queue(issues, active_issue);

        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].number, 21);
        assert!(queue[0].is_active);
    }

    #[test]
    fn test_get_auto_worker_queue_uses_cached_issues() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());
        let project_id = Uuid::new_v4();
        let repo_path = projects_root.path().join("queue-project");
        fs::create_dir_all(&repo_path).expect("create repo dir");

        {
            let storage = app_state.storage.lock().expect("lock storage");
            storage
                .save_project(&crate::models::Project {
                    id: project_id,
                    name: "Queue Project".to_string(),
                    repo_path: repo_path.to_string_lossy().to_string(),
                    created_at: "2026-03-10T00:00:00Z".to_string(),
                    archived: false,
                    sessions: vec![crate::models::SessionConfig {
                        id: Uuid::new_v4(),
                        label: "session-1".to_string(),
                        worktree_path: None,
                        worktree_branch: None,
                        archived: false,
                        kind: "claude".to_string(),
                        github_issue: Some(make_issue(
                            33,
                            "Active worker issue",
                            &["priority:high", "complexity:low", "assigned-to-auto-worker"],
                        )),
                        initial_prompt: None,
                        done_commits: vec![],
                        auto_worker_session: true,
                    }],
                    maintainer: crate::models::MaintainerConfig::default(),
                    auto_worker: crate::models::AutoWorkerConfig { enabled: true },
                    prompts: vec![],
                    staged_sessions: vec![],
                })
                .expect("save project");
        }

        {
            let mut issue_cache = app_state.issue_cache.lock().expect("lock cache");
            issue_cache.insert(
                repo_path.to_string_lossy().to_string(),
                vec![
                    make_issue(
                        33,
                        "Active worker issue",
                        &["priority:high", "complexity:low"],
                    ),
                    make_issue(34, "Queued issue", &["priority:high", "complexity:low"]),
                    make_issue(35, "Ineligible issue", &["priority:high"]),
                ],
            );
        }

        let queue = run_async_test(service::get_auto_worker_queue(&app_state, project_id))
            .map_err(|e| e.to_string())
            .expect("queue command should succeed");

        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].number, 33);
        assert!(queue[0].is_active);
        assert_eq!(queue[1].number, 34);
        assert!(!queue[1].is_active);
    }

    #[test]
    fn test_submit_secure_env_value_command_writes_env_file() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());

        let project = service::create_project(
            &app_state,
            "secure-env-submit",
            &repo_dir.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect("create project");

        crate::secure_env::begin_secure_env_request(
            &app_state,
            &project.name,
            "OPENAI_API_KEY",
            "req-123",
        )
        .expect("begin secure env request");

        let status = run_async_test(service::submit_secure_env_value(
            &app_state,
            "req-123",
            "new-secret",
        ))
        .map_err(|e| e.to_string())
        .expect("submit secure env value");

        assert_eq!(status, "created");
        let written = fs::read_to_string(repo_dir.path().join(".env")).expect("read .env");
        assert_eq!(written, "OPENAI_API_KEY=new-secret\n");
    }

    #[test]
    fn test_cancel_secure_env_request_command_clears_pending_request() {
        let base_dir = TempDir::new().unwrap();
        let projects_root = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let app_state = make_test_state(base_dir.path(), projects_root.path());

        let project = service::create_project(
            &app_state,
            "secure-env-cancel",
            &repo_dir.path().to_string_lossy(),
        )
        .map_err(|e| e.to_string())
        .expect("create project");

        crate::secure_env::begin_secure_env_request(
            &app_state,
            &project.name,
            "OPENAI_API_KEY",
            "req-123",
        )
        .expect("begin secure env request");

        service::cancel_secure_env_request(&app_state, "req-123")
            .map_err(|e| e.to_string())
            .expect("cancel secure env request");

        assert!(app_state.secure_env_request.lock().unwrap().is_none());
    }
}

#[cfg(test)]
mod staging_tests {
    use super::*;

    #[test]
    fn test_find_free_port_returns_offset_port_when_free() {
        // Port 59123 is unlikely to be in use
        let port = find_staging_port(58123).unwrap();
        assert_eq!(port, 59123); // base + 1000
    }

    #[test]
    fn test_find_free_port_skips_occupied() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let occupied_port = listener.local_addr().unwrap().port();
        let Some(base) = occupied_port.checked_sub(STAGING_PORT_OFFSET) else {
            return; // OS assigned a port too low to construct a valid base
        };
        let port = find_staging_port(base).unwrap();
        assert!(port > occupied_port);
        assert!(port <= occupied_port + 100);
    }

    #[test]
    fn test_find_staging_port_checks_ipv6() {
        // Bind on IPv6 only — find_staging_port must detect this.
        // Skip gracefully on systems without IPv6 loopback.
        let Ok(listener) = std::net::TcpListener::bind("[::1]:0") else {
            return;
        };
        let occupied_port = listener.local_addr().unwrap().port();
        let base = occupied_port.checked_sub(STAGING_PORT_OFFSET).unwrap();
        let port = find_staging_port(base).unwrap();
        assert_ne!(port, occupied_port, "must skip port occupied on IPv6");
    }
}
