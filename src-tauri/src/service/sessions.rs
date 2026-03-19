use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::emitter::EventEmitter;
use crate::error::AppError;
use crate::models::{CommitInfo, GithubIssue, SavedPrompt, SessionConfig, StagedSession};
use crate::pty_manager::PtyManager;
use crate::state::AppState;
use crate::storage::Storage;
use crate::token_usage::{self, TokenDataPoint};
use crate::worktree::WorktreeManager;

// ---------------------------------------------------------------------------
// Session management helpers (moved from commands.rs)
// ---------------------------------------------------------------------------

/// Generate the next session label by finding the highest existing session number
/// and returning "session-N-<6-char-uuid>" where N = max + 1. The UUID suffix
/// guarantees uniqueness even when branches from deleted sessions persist on the
/// remote.
pub fn next_session_label(sessions: &[SessionConfig]) -> String {
    let max_num = sessions
        .iter()
        .filter_map(|s| s.label.strip_prefix("session-"))
        .filter_map(|n| n.split('-').next()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    let short_id = &Uuid::new_v4().to_string()[..6];
    format!("session-{}-{}", max_num + 1, short_id)
}

pub(crate) fn cleanup_failed_session_spawn(
    repo_path: &str,
    worktree_path: Option<&str>,
    worktree_branch: Option<&str>,
) -> Result<(), String> {
    if let Some((path, branch)) = worktree_path.zip(worktree_branch) {
        WorktreeManager::remove_worktree(path, repo_path, branch)?;
    }
    Ok(())
}

pub(crate) const STAGING_PORT_OFFSET: u16 = 1000;

/// Find a free port for the staged Controller instance.
/// Starts at base_port + 1000 and increments until a free port is found.
pub(crate) fn find_staging_port(base_port: u16) -> Result<u16, String> {
    let start = base_port
        .checked_add(STAGING_PORT_OFFSET)
        .ok_or("Port overflow")?;
    for candidate in start..start.saturating_add(100) {
        let ipv4_free = std::net::TcpListener::bind(("127.0.0.1", candidate)).is_ok();
        let ipv6_free = std::net::TcpListener::bind(("::1", candidate)).is_ok();
        if ipv4_free && ipv6_free {
            return Ok(candidate);
        }
    }
    Err(format!(
        "No free port found in range {}-{}",
        start,
        start.saturating_add(99)
    ))
}

/// Kill a process group by PID. Sends SIGTERM to the group, then SIGKILL after 2s
/// if the group is still alive.
pub fn kill_process_group(pid: u32) {
    if pid == 0 {
        tracing::warn!("refusing to kill process group 0 (would kill own process group)");
        return;
    }
    tracing::debug!(pid, "killing process group");
    #[cfg(unix)]
    {
        use libc::{kill, SIGKILL, SIGTERM};
        if let Ok(pgid) = i32::try_from(pid) {
            unsafe {
                kill(-pgid, SIGTERM);
            }
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                // Only send SIGKILL if the process group is still alive
                // (kill with signal 0 checks existence without sending a signal)
                if unsafe { kill(-pgid, 0) } == 0 {
                    unsafe {
                        kill(-pgid, SIGKILL);
                    }
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Session management service functions
// ---------------------------------------------------------------------------

/// Run storage migrations on startup (worktree path format, etc.).
/// PTY connections are deferred to `connect_session` so each terminal
/// can attach at the correct size.
///
/// Takes `&Arc<Mutex<Storage>>` so callers can clone it for `spawn_blocking`.
pub fn restore_sessions(storage: &Arc<Mutex<Storage>>) -> Result<(), AppError> {
    tracing::info!("restoring sessions from storage");
    let storage = storage.lock().map_err(AppError::internal)?;
    let inventory = storage.list_projects().map_err(AppError::internal)?;
    inventory.warn_if_corrupt("restore_sessions");
    // Migrate worktree paths from UUID-based to name-based directories
    for project in &inventory.projects {
        if let Err(e) = storage.migrate_worktree_paths(project) {
            tracing::error!(
                "failed to migrate worktrees for project '{}': {}",
                project.name,
                e
            );
        }
    }
    Ok(())
}

/// Connect a terminal to its PTY session at the given size.
/// This is synchronous — callers that need non-blocking behaviour should
/// wrap in `spawn_blocking`.
///
/// Takes individual `Arc` fields instead of `&AppState` so that callers can
/// clone them before entering a `spawn_blocking` closure.
pub fn connect_session(
    storage: &Arc<Mutex<Storage>>,
    pty_manager: &Arc<Mutex<PtyManager>>,
    emitter: &Arc<dyn EventEmitter>,
    session_id: Uuid,
    rows: u16,
    cols: u16,
) -> Result<(), AppError> {
    // Check if already connected
    {
        let pty_manager = pty_manager.lock().map_err(AppError::internal)?;
        if pty_manager.sessions.contains_key(&session_id) {
            tracing::debug!(session_id = %session_id, "session already connected, skipping");
            return Ok(());
        }
    }
    tracing::info!(session_id = %session_id, rows, cols, "connecting session to PTY");

    // Find session config from storage
    let (session_dir, kind) = {
        let storage = storage.lock().map_err(AppError::internal)?;
        let inventory = storage.list_projects().map_err(AppError::internal)?;
        inventory.warn_if_corrupt("connect_session");
        inventory
            .projects
            .iter()
            .flat_map(|p| p.sessions.iter().map(move |s| (p, s)))
            .find(|(_, s)| s.id == session_id)
            .map(|(p, s)| {
                let dir = s
                    .worktree_path
                    .clone()
                    .unwrap_or_else(|| p.repo_path.clone());
                (dir, s.kind.clone())
            })
            .ok_or_else(|| AppError::NotFound(format!("session not found: {}", session_id)))?
    };

    let mut mgr = pty_manager.lock().map_err(AppError::internal)?;
    // Definitive check under lock to prevent double-spawn race (TOCTOU)
    if mgr.sessions.contains_key(&session_id) {
        tracing::debug!(session_id = %session_id, "session connected by concurrent request, skipping");
        return Ok(());
    }
    mgr.spawn_session(
        session_id,
        &session_dir,
        &kind,
        emitter.clone(),
        true,
        None,
        rows,
        cols,
    )
    .map_err(AppError::Internal)
}

/// Create a new session. This is synchronous (blocking) — callers that need
/// non-blocking behaviour should wrap in `spawn_blocking`.
///
/// Takes individual `Arc` fields instead of `&AppState` so that callers can
/// clone them before entering a `spawn_blocking` closure.
#[allow(clippy::too_many_arguments)]
pub fn create_session(
    storage: &Arc<Mutex<Storage>>,
    pty_manager: &Arc<Mutex<PtyManager>>,
    emitter: &Arc<dyn EventEmitter>,
    project_id: Uuid,
    session_id: Uuid,
    kind: &str,
    github_issue: Option<GithubIssue>,
    background: bool,
    initial_prompt: Option<String>,
) -> Result<String, AppError> {
    tracing::info!(
        session_id = %session_id,
        project_id = %project_id,
        kind = %kind,
        background,
        has_github_issue = github_issue.is_some(),
        has_initial_prompt = initial_prompt.is_some(),
        "creating session"
    );

    // Load the project and generate session label
    let (repo_path, label, base_dir, project_name) = {
        let storage = storage.lock().map_err(AppError::internal)?;
        let project = storage
            .load_project(project_id)
            .map_err(AppError::internal)?;
        let label = next_session_label(&project.sessions);
        (
            project.repo_path.clone(),
            label,
            storage.base_dir(),
            project.name.clone(),
        )
    };

    // Create worktree under ~/.the-controller/worktrees/{project_name}/{label}/
    let worktree_dir = base_dir.join("worktrees").join(&project_name).join(&label);

    // Try to create a worktree; fall back to repo path for repos without commits
    let (session_dir, wt_path, wt_branch) =
        match WorktreeManager::create_worktree(&repo_path, &label, &worktree_dir) {
            Ok(worktree_path) => {
                let wt_str = worktree_path
                    .to_str()
                    .ok_or_else(|| {
                        AppError::Internal("worktree path is not valid UTF-8".to_string())
                    })?
                    .to_string();
                (wt_str.clone(), Some(wt_str), Some(label.clone()))
            }
            Err(e) if e == "unborn_branch" => {
                // Repo has no commits — use repo path directly, no worktree
                (repo_path.clone(), None, None)
            }
            Err(e) => return Err(AppError::Internal(e)),
        };

    // Build initial prompt: explicit prompt takes priority, then GitHub issue context
    let initial_prompt = initial_prompt.or_else(|| {
        github_issue.as_ref().map(|issue| {
            crate::session_args::build_issue_prompt(
                issue.number,
                &issue.title,
                &issue.url,
                background,
            )
        })
    });
    let rollback_worktree = wt_path.clone().zip(wt_branch.clone());

    let session_config = SessionConfig {
        id: session_id,
        label: label.clone(),
        worktree_path: wt_path,
        worktree_branch: wt_branch,
        archived: false,
        kind: kind.to_string(),
        github_issue,
        initial_prompt: initial_prompt.clone(),
        done_commits: vec![],
        auto_worker_session: false,
    };

    // Save session config to storage, then spawn PTY (with rollback on failure)
    {
        let storage = storage.lock().map_err(AppError::internal)?;
        let mut project = storage
            .load_project(project_id)
            .map_err(AppError::internal)?;
        project.sessions.push(session_config);
        storage.save_project(&project).map_err(AppError::internal)?;
    }

    let spawn_result = {
        let mut mgr = pty_manager.lock().map_err(AppError::internal)?;
        mgr.spawn_session(
            session_id,
            &session_dir,
            kind,
            emitter.clone(),
            false,
            initial_prompt.as_deref(),
            24,
            80,
        )
    };

    if let Err(ref spawn_err) = spawn_result {
        tracing::error!(session_id = %session_id, error = %spawn_err, "session PTY spawn failed, rolling back");
        // Rollback: remove session from storage
        if let Ok(storage) = storage.lock() {
            if let Ok(mut project) = storage.load_project(project_id) {
                project.sessions.retain(|session| session.id != session_id);
                let _ = storage.save_project(&project);
            }
        }
        // Clean up worktree on spawn failure
        if let Some((ref worktree_path, ref worktree_branch)) = rollback_worktree {
            if let Err(cleanup_err) = cleanup_failed_session_spawn(
                &repo_path,
                Some(worktree_path.as_str()),
                Some(worktree_branch.as_str()),
            ) {
                return Err(AppError::Internal(format!(
                    "{} (worktree cleanup failed: {})",
                    spawn_err, cleanup_err
                )));
            }
        }
    }

    spawn_result.map_err(AppError::Internal)?;
    Ok(session_id.to_string())
}

/// Close a session. Closes PTY, removes session from project, optionally
/// removes worktree. This is synchronous.
pub fn close_session(
    state: &AppState,
    project_id: Uuid,
    session_id: Uuid,
    delete_worktree: bool,
) -> Result<(), AppError> {
    tracing::info!(session_id = %session_id, project_id = %project_id, delete_worktree, "closing session");

    // Try to close the PTY session even if the terminal is already gone.
    {
        let mut pty_manager = state.pty_manager.lock().map_err(AppError::internal)?;
        let _ = pty_manager.close_session(session_id);
    }

    // Remove session from project
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let mut project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;

    let session = project
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .cloned();
    project.sessions.retain(|s| s.id != session_id);
    storage.save_project(&project).map_err(AppError::internal)?;

    // Optionally clean up worktree
    if delete_worktree {
        if let Some(session) = session {
            if let (Some(wt_path), Some(branch)) = (session.worktree_path, session.worktree_branch)
            {
                let _ = WorktreeManager::remove_worktree(&wt_path, &project.repo_path, &branch);
            }
        }
    }

    Ok(())
}

/// Write data to a PTY session.
pub fn write_to_pty(state: &AppState, session_id: Uuid, data: &[u8]) -> Result<(), AppError> {
    let mut pty_manager = state.pty_manager.lock().map_err(AppError::internal)?;
    pty_manager
        .write_to_session(session_id, data)
        .map_err(AppError::Internal)
}

/// Send raw data to a PTY session.
pub fn send_raw_to_pty(state: &AppState, session_id: Uuid, data: &[u8]) -> Result<(), AppError> {
    let mut pty_manager = state.pty_manager.lock().map_err(AppError::internal)?;
    pty_manager
        .send_raw_to_session(session_id, data)
        .map_err(AppError::Internal)
}

/// Resize a PTY session.
pub fn resize_pty(
    state: &AppState,
    session_id: Uuid,
    rows: u16,
    cols: u16,
) -> Result<(), AppError> {
    tracing::debug!(session_id = %session_id, rows, cols, "resizing PTY");
    let pty_manager = state.pty_manager.lock().map_err(AppError::internal)?;
    pty_manager
        .resize_session(session_id, rows, cols)
        .map_err(AppError::Internal)
}

const COMMIT_POLL_INTERVAL_SECS: u64 = 3;
const MAX_COMMIT_WAIT_SECS: u64 = 60;
const MAX_REBASE_WAIT_SECS: u64 = 360; // 6 minutes
const REBASE_POLL_INTERVAL_SECS: u64 = 3;

/// Core staging logic. Returns the port on success.
///
/// When `allow_pty_prompts` is true (Tauri command path), dirty worktrees and
/// rebase conflicts are handled by prompting the session's Claude via PTY.
/// When false (socket path), these conditions return an error immediately —
/// the caller is expected to commit and resolve conflicts before staging.
pub async fn stage_session_core(
    state: &AppState,
    project_id: Uuid,
    session_id: Uuid,
    allow_pty_prompts: bool,
) -> Result<u16, AppError> {
    use std::process::Stdio;

    tracing::info!(session_id = %session_id, project_id = %project_id, "staging session");

    let _staging_guard = state.staging_lock.lock().await;

    // Extract data under a short-lived storage lock to avoid deadlock with pty_manager
    let (repo_path, branch, worktree_path) = {
        let storage = state.storage.lock().map_err(AppError::internal)?;
        let project = storage
            .load_project(project_id)
            .map_err(AppError::internal)?;

        if project.name != "the-controller" {
            tracing::warn!(project_name = %project.name, "staging rejected: only supported for the-controller");
            return Err(AppError::BadRequest(
                "Staging is only supported for the-controller".to_string(),
            ));
        }

        // Check if this specific session is already staged
        if let Some(existing) = project
            .staged_sessions
            .iter()
            .find(|s| s.session_id == session_id)
        {
            #[cfg(unix)]
            let alive = i32::try_from(existing.pid)
                .map(|pid| unsafe { libc::kill(pid, 0) } == 0)
                .unwrap_or(false);
            #[cfg(not(unix))]
            let alive = false;
            if alive {
                tracing::warn!(
                    pid = existing.pid,
                    "stage_session: session already staged and alive"
                );
                return Err(AppError::BadRequest(
                    "This session is already staged — unstage it first".to_string(),
                ));
            }
            // Stale record — clean up
            kill_process_group(existing.pid);
            let stale_socket = crate::status_socket::staged_socket_path(&session_id);
            let _ = std::fs::remove_file(&stale_socket);
            let mut p = project.clone();
            p.staged_sessions.retain(|s| s.session_id != session_id);
            storage.save_project(&p).map_err(AppError::internal)?;
        }

        let session = project
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| AppError::NotFound("Session not found".to_string()))?;

        let branch = session
            .worktree_branch
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("Session has no worktree branch".to_string()))?
            .to_string();

        let worktree_path = session
            .worktree_path
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("Session has no worktree path".to_string()))?
            .to_string();

        (project.repo_path.clone(), branch, worktree_path)
    };

    // 1. Ensure worktree is clean
    {
        let wt = worktree_path.clone();
        let is_clean = tokio::task::spawn_blocking(move || WorktreeManager::is_worktree_clean(&wt))
            .await
            .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
            .map_err(AppError::Internal)?;

        if !is_clean {
            if !allow_pty_prompts {
                return Err(AppError::BadRequest(
                    "Worktree has uncommitted changes — commit before staging".to_string(),
                ));
            }

            let prompt = "\nYou have uncommitted changes. Please commit all your work now.\r";
            {
                let mut pty_manager = state.pty_manager.lock().map_err(AppError::internal)?;
                let _ = pty_manager.write_to_session(session_id, prompt.as_bytes());
            }

            let _ = state
                .emitter
                .emit("staging-status", "Waiting for commit...");

            let max_polls = MAX_COMMIT_WAIT_SECS / COMMIT_POLL_INTERVAL_SECS;
            let mut committed = false;
            for _ in 0..max_polls {
                tokio::time::sleep(std::time::Duration::from_secs(COMMIT_POLL_INTERVAL_SECS)).await;
                let wt_check = worktree_path.clone();
                let clean = tokio::task::spawn_blocking(move || {
                    WorktreeManager::is_worktree_clean(&wt_check)
                })
                .await
                .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
                .map_err(AppError::Internal)?;
                if clean {
                    committed = true;
                    break;
                }
            }
            if !committed {
                tracing::error!(session_id = %session_id, "stage_session: timed out waiting for commit");
                return Err(AppError::Internal(
                    "Timed out waiting for commit. Please commit manually and retry.".to_string(),
                ));
            }
        }
    }

    // 2. Rebase onto main if needed
    {
        let rp = repo_path.clone();
        let main_branch =
            tokio::task::spawn_blocking(move || WorktreeManager::detect_main_branch(&rp))
                .await
                .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
                .map_err(AppError::Internal)?;

        let rp = repo_path.clone();
        let mb2 = main_branch.clone();
        let _ = tokio::task::spawn_blocking(move || WorktreeManager::sync_main(&rp, &mb2)).await;

        let rp = repo_path.clone();
        let br = branch.clone();
        let mb = main_branch.clone();
        let is_behind =
            tokio::task::spawn_blocking(move || WorktreeManager::is_branch_behind(&rp, &br, &mb))
                .await
                .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
                .map_err(AppError::Internal)?;

        if is_behind {
            let wt = worktree_path.clone();
            let mb = main_branch.clone();
            let rebase_clean =
                tokio::task::spawn_blocking(move || WorktreeManager::rebase_onto(&wt, &mb))
                    .await
                    .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
                    .map_err(AppError::Internal)?;

            if !rebase_clean {
                if !allow_pty_prompts {
                    return Err(AppError::BadRequest(
                        "Rebase has conflicts — resolve before staging".to_string(),
                    ));
                }

                // Rebase has conflicts — ask Claude to resolve
                let prompt = "\nThere are rebase conflicts. Please resolve all conflicts, then run `git rebase --continue`.\r";
                {
                    let mut pty_manager = state.pty_manager.lock().map_err(AppError::internal)?;
                    let _ = pty_manager.write_to_session(session_id, prompt.as_bytes());
                }

                let _ = state
                    .emitter
                    .emit("staging-status", "Rebase conflicts. Claude is resolving...");

                // Poll until rebase is no longer in progress
                let max_polls = MAX_REBASE_WAIT_SECS / REBASE_POLL_INTERVAL_SECS;
                let mut resolved = false;
                for _ in 0..max_polls {
                    tokio::time::sleep(std::time::Duration::from_secs(REBASE_POLL_INTERVAL_SECS))
                        .await;
                    let wt_check = worktree_path.clone();
                    let still_rebasing = tokio::task::spawn_blocking(move || {
                        WorktreeManager::is_rebase_in_progress(&wt_check)
                    })
                    .await
                    .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?;
                    if !still_rebasing {
                        resolved = true;
                        break;
                    }
                }
                if !resolved {
                    tracing::error!(session_id = %session_id, "stage_session: timed out waiting for rebase conflict resolution");
                    return Err(AppError::Internal(
                        "Timed out waiting for rebase conflict resolution.".to_string(),
                    ));
                }
            }
        }
    }

    // 3. Launch a separate Controller instance from the worktree
    let _ = state
        .emitter
        .emit("staging-status", "Preparing staged instance...");

    // Ensure node_modules exists in the worktree
    let node_modules = PathBuf::from(&worktree_path).join("node_modules");
    if !node_modules.exists() {
        let _ = state
            .emitter
            .emit("staging-status", "Installing dependencies...");
        let wt = worktree_path.clone();
        let install_status = tokio::task::spawn_blocking(move || {
            std::process::Command::new("pnpm")
                .arg("install")
                .current_dir(&wt)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
        })
        .await
        .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
        .map_err(|e| AppError::Internal(format!("pnpm install failed: {}", e)))?;

        if !install_status.success() {
            return Err(AppError::Internal(
                "pnpm install failed in worktree".to_string(),
            ));
        }
    }

    let port = find_staging_port(1420).map_err(AppError::Internal)?;

    let _ = state
        .emitter
        .emit("staging-status", &format!("Starting on port {}...", port));

    let wt = worktree_path.clone();
    let log_path = PathBuf::from(&wt).join("staging.log");
    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| AppError::Internal(format!("Failed to create staging log: {}", e)))?;
    let log_stderr = log_file
        .try_clone()
        .map_err(|e| AppError::Internal(format!("Failed to clone log file: {}", e)))?;

    #[cfg(unix)]
    let mut child = {
        use std::os::unix::process::CommandExt;
        std::process::Command::new("bash")
            .args(["./dev.sh", &port.to_string()])
            .current_dir(&wt)
            .env(
                "CONTROLLER_SOCKET",
                crate::status_socket::staged_socket_path(&session_id),
            )
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_stderr))
            .process_group(0)
            .spawn()
            .map_err(|e| AppError::Internal(format!("Failed to spawn staged instance: {}", e)))?
    };

    #[cfg(not(unix))]
    let mut child = std::process::Command::new("bash")
        .args(["./dev.sh", &port.to_string()])
        .current_dir(&wt)
        .env(
            "CONTROLLER_SOCKET",
            crate::status_socket::staged_socket_path(&session_id),
        )
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_stderr))
        .spawn()
        .map_err(|e| AppError::Internal(format!("Failed to spawn staged instance: {}", e)))?;

    let pid = child.id();
    tracing::info!(session_id = %session_id, pid, port, "staged instance spawned");
    // Reap the child in a background thread to prevent zombie entries.
    // We manage the process lifetime via PID/process group (kill_process_group),
    // not via this Child handle.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    // Save staged session info — if save fails, kill the orphan process
    let save_result = (|| -> Result<(), AppError> {
        let storage = state.storage.lock().map_err(AppError::internal)?;
        let mut project = storage
            .load_project(project_id)
            .map_err(AppError::internal)?;

        project.staged_sessions.push(StagedSession {
            session_id,
            pid,
            port,
        });

        storage.save_project(&project).map_err(AppError::internal)
    })();

    if let Err(e) = save_result {
        tracing::error!(pid, error = %e, "stage_session: failed to save staged session, killing orphan process");
        kill_process_group(pid);
        return Err(e);
    }

    Ok(port)
}

/// Unstage a session: kill the staged process and remove the staged record.
pub fn unstage_session(
    state: &AppState,
    project_id: Uuid,
    session_id: Uuid,
) -> Result<(), AppError> {
    tracing::info!(project_id = %project_id, "unstaging session");

    let storage = state.storage.lock().map_err(AppError::internal)?;
    let mut project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;

    let idx = project
        .staged_sessions
        .iter()
        .position(|s| s.session_id == session_id)
        .ok_or_else(|| AppError::BadRequest("This session is not currently staged".to_string()))?;

    let staged = project.staged_sessions.remove(idx);

    // Kill the staged Controller process group
    kill_process_group(staged.pid);

    // Clean up this session's socket
    let socket = crate::status_socket::staged_socket_path(&session_id);
    let _ = std::fs::remove_file(&socket);

    storage.save_project(&project).map_err(AppError::internal)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Git / repo helpers
// ---------------------------------------------------------------------------

/// Get the HEAD branch name and short commit hash for a repository.
pub fn get_repo_head(repo_path: &str) -> Result<(String, String), AppError> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|e| AppError::Internal(format!("Failed to open repo: {}", e)))?;
    let head = repo
        .head()
        .map_err(|e| AppError::Internal(format!("Failed to get HEAD: {}", e)))?;
    let branch = head.shorthand().unwrap_or("HEAD").to_string();
    let commit = head
        .peel_to_commit()
        .map_err(|e| AppError::Internal(format!("Failed to peel to commit: {}", e)))?;
    let short_hash = commit.id().to_string()[..7].to_string();
    Ok((branch, short_hash))
}

/// Find the OID of the main (or master) branch in a repository.
pub fn find_main_branch_oid(repo: &git2::Repository) -> Option<git2::Oid> {
    for name in &["refs/heads/main", "refs/heads/master"] {
        if let Ok(reference) = repo.find_reference(name) {
            if let Ok(commit) = reference.peel_to_commit() {
                return Some(commit.id());
            }
        }
    }
    None
}

/// Walk commits on the worktree branch that aren't on the main branch.
pub fn discover_branch_commits(worktree_path: &str) -> Result<Vec<CommitInfo>, String> {
    let repo = git2::Repository::discover(worktree_path)
        .map_err(|e| format!("Failed to open repo: {e}"))?;
    let head = repo.head().map_err(|e| format!("No HEAD: {e}"))?;
    let head_commit = head.peel_to_commit().map_err(|e| e.to_string())?;
    let main_oid = find_main_branch_oid(&repo);
    let mut revwalk = repo.revwalk().map_err(|e| e.to_string())?;
    revwalk.push(head_commit.id()).map_err(|e| e.to_string())?;
    revwalk
        .set_sorting(git2::Sort::TOPOLOGICAL)
        .map_err(|e| e.to_string())?;
    let mut commits = Vec::new();
    for oid in revwalk {
        let oid = oid.map_err(|e| e.to_string())?;
        if let Some(main) = main_oid {
            if oid == main {
                break;
            }
            if let Ok(base) = repo.merge_base(oid, main) {
                if base == oid {
                    break;
                }
            }
        }
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let message = commit.summary().unwrap_or("").to_string();
        if message.starts_with("Initial commit") {
            continue;
        }
        let hash = oid.to_string()[..7].to_string();
        commits.push(CommitInfo { hash, message });
        if commits.len() >= 20 {
            break;
        }
    }
    Ok(commits)
}

// ---------------------------------------------------------------------------
// Session data queries
// ---------------------------------------------------------------------------

/// Get commits for a session, merging stored done_commits with newly discovered ones.
pub fn get_session_commits(
    storage: &Arc<Mutex<Storage>>,
    project_id: Uuid,
    session_id: Uuid,
) -> Result<Vec<CommitInfo>, AppError> {
    let storage = storage.lock().map_err(AppError::internal)?;
    let project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;
    let session = project
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| AppError::NotFound("Session not found".to_string()))?;

    let worktree_path = match &session.worktree_path {
        Some(p) => p.clone(),
        None => return Ok(session.done_commits.clone()),
    };

    let new_commits = discover_branch_commits(&worktree_path).unwrap_or_default();

    let mut seen = std::collections::HashSet::new();
    let mut all_commits = Vec::new();
    for c in new_commits.iter().chain(session.done_commits.iter()) {
        if seen.insert(c.hash.clone()) {
            all_commits.push(c.clone());
        }
    }

    if all_commits.len() > session.done_commits.len() {
        let mut project = project.clone();
        if let Some(s) = project.sessions.iter_mut().find(|s| s.id == session_id) {
            s.done_commits = all_commits.clone();
        }
        let _ = storage.save_project(&project);
    }

    Ok(all_commits)
}

/// Get token usage data for a session.
pub fn get_session_token_usage(
    storage: &Arc<Mutex<Storage>>,
    project_id: Uuid,
    session_id: Uuid,
) -> Result<Vec<TokenDataPoint>, AppError> {
    let storage = storage.lock().map_err(AppError::internal)?;
    let project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;
    let session = project
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| AppError::NotFound("Session not found".to_string()))?;

    let working_dir = session
        .worktree_path
        .as_deref()
        .unwrap_or(&project.repo_path);

    token_usage::get_token_usage(working_dir, &session.kind).map_err(AppError::Internal)
}

// ---------------------------------------------------------------------------
// Prompt management
// ---------------------------------------------------------------------------

/// Save the current session's prompt as a reusable project prompt.
pub fn save_session_prompt(
    state: &AppState,
    project_id: Uuid,
    session_id: Uuid,
) -> Result<(), AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let mut project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;
    let session = project
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| AppError::NotFound("Session not found".to_string()))?;

    let prompt_text = session
        .initial_prompt
        .clone()
        .or_else(|| {
            session.github_issue.as_ref().map(|issue| {
                crate::session_args::build_issue_prompt(
                    issue.number,
                    &issue.title,
                    &issue.url,
                    false,
                )
            })
        })
        .ok_or_else(|| AppError::BadRequest("Session has no prompt to save".to_string()))?;

    let name = {
        let truncated: String = prompt_text.chars().take(60).collect();
        if truncated.chars().count() < prompt_text.chars().count() {
            format!("{}...", truncated)
        } else {
            truncated
        }
    };

    let saved = SavedPrompt {
        id: Uuid::new_v4(),
        name,
        text: prompt_text,
        created_at: chrono::Utc::now().to_rfc3339(),
        source_session_label: session.label.clone(),
    };

    project.prompts.push(saved);
    storage.save_project(&project).map_err(AppError::internal)?;
    Ok(())
}

/// List saved prompts for a project.
pub fn list_project_prompts(
    state: &AppState,
    project_id: Uuid,
) -> Result<Vec<SavedPrompt>, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;
    Ok(project.prompts)
}

// ---------------------------------------------------------------------------
// Merge session branch
// ---------------------------------------------------------------------------

/// Wait for a rebase to finish by polling a closure.
pub async fn wait_for_merge_rebase_resolution<F, Fut>(
    mut is_rebase_in_progress: F,
    max_polls: u64,
    poll_interval: std::time::Duration,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool, String>>,
{
    for _ in 0..max_polls {
        tokio::time::sleep(poll_interval).await;
        if !is_rebase_in_progress().await? {
            return Ok(());
        }
    }
    Err("Timed out waiting for merge conflict resolution.".to_string())
}

const MAX_MERGE_RETRIES: u32 = 5;
const MERGE_REBASE_POLL_INTERVAL_SECS: u64 = 3;
const MAX_MERGE_REBASE_WAIT_SECS: u64 = 600; // 10 minutes

/// Merge a session branch back to main via PR, with retry logic for rebase conflicts.
/// When `allow_pty_prompts` is true, sends prompts to Claude to resolve conflicts.
pub async fn merge_session_branch(
    state: &AppState,
    project_id: Uuid,
    session_id: Uuid,
    allow_pty_prompts: bool,
) -> Result<crate::models::MergeResponse, AppError> {
    tracing::info!(session_id = %session_id, project_id = %project_id, "merging session branch");

    let (repo_path, worktree_path, branch_name) = {
        let storage = state.storage.lock().map_err(AppError::internal)?;
        let project = storage
            .load_project(project_id)
            .map_err(AppError::internal)?;
        let session = project
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| AppError::NotFound("Session not found".to_string()))?;
        let wt_path = session
            .worktree_path
            .clone()
            .ok_or_else(|| AppError::BadRequest("Session has no worktree".to_string()))?;
        let branch = session
            .worktree_branch
            .clone()
            .ok_or_else(|| AppError::BadRequest("Session has no branch".to_string()))?;
        (project.repo_path.clone(), wt_path, branch)
    };

    for attempt in 0..MAX_MERGE_RETRIES {
        let rp = repo_path.clone();
        let wt = worktree_path.clone();
        let br = branch_name.clone();

        let result = tokio::task::spawn_blocking(move || {
            if WorktreeManager::is_rebase_in_progress(&wt) {
                Ok(crate::worktree::MergeResult::RebaseConflicts)
            } else {
                WorktreeManager::merge_via_pr(&rp, &wt, &br)
            }
        })
        .await
        .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
        .map_err(AppError::Internal)?;

        match result {
            crate::worktree::MergeResult::PrCreated(url) => {
                tracing::info!(session_id = %session_id, url = %url, "PR created");
                return Ok(crate::models::MergeResponse::PrCreated { url });
            }
            crate::worktree::MergeResult::RebaseConflicts => {
                tracing::warn!(session_id = %session_id, attempt = attempt + 1, max = MAX_MERGE_RETRIES, "rebase conflicts during merge");

                if allow_pty_prompts {
                    let prompt = "merge\r";
                    {
                        let mut pty_manager =
                            state.pty_manager.lock().map_err(AppError::internal)?;
                        let _ = pty_manager.write_to_session(session_id, prompt.as_bytes());
                    }
                }

                let _ = state.emitter.emit(
                    "merge-status",
                    &format!(
                        "Rebase conflicts (attempt {}/{}). Claude is resolving...",
                        attempt + 1,
                        MAX_MERGE_RETRIES
                    ),
                );

                let wt_poll = worktree_path.clone();
                wait_for_merge_rebase_resolution(
                    move || {
                        let wt_check = wt_poll.clone();
                        async move {
                            tokio::task::spawn_blocking(move || {
                                WorktreeManager::is_rebase_in_progress(&wt_check)
                            })
                            .await
                            .map_err(|e| format!("Task failed: {}", e))
                        }
                    },
                    MAX_MERGE_REBASE_WAIT_SECS / MERGE_REBASE_POLL_INTERVAL_SECS,
                    std::time::Duration::from_secs(MERGE_REBASE_POLL_INTERVAL_SECS),
                )
                .await
                .map_err(AppError::Internal)?;

                continue;
            }
        }
    }

    Err(AppError::Internal(format!(
        "Merge failed after {} attempts due to recurring conflicts",
        MAX_MERGE_RETRIES
    )))
}

/// Set the initial prompt for a session (only if not already set).
pub fn set_initial_prompt(
    state: &AppState,
    project_id: Uuid,
    session_id: Uuid,
    prompt: String,
) -> Result<(), AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let mut project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;

    if let Some(session) = project.sessions.iter_mut().find(|s| s.id == session_id) {
        if session.initial_prompt.is_none() {
            session.initial_prompt = Some(prompt);
            storage.save_project(&project).map_err(AppError::internal)?;
        }
    }

    Ok(())
}
