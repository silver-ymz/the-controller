//! Service layer — shared business logic for Tauri commands and Axum handlers.
//!
//! Each public function in this module (and its submodules) encapsulates a
//! single unit of business logic. Both `commands.rs` (Tauri IPC) and
//! `server/` (Axum HTTP) delegate here, keeping the API surfaces thin.
//!
//! Errors are returned as [`crate::error::AppError`], which converts into
//! the appropriate response type for each API surface.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::config;
use crate::deploy::commands::{DeployRequest, DeployResult, ProjectSignals};
use crate::deploy::coolify::CoolifyClient;
use crate::deploy::credentials::DeployCredentials;
use crate::emitter::EventEmitter;
use crate::error::AppError;
use crate::keybindings;
use crate::models::{
    AssignedIssue, GithubIssue, MaintainerIssue, MaintainerIssueDetail, Project, SessionConfig,
    StagedSession,
};
use crate::note_ai_chat::{NoteAiChatMessage, NoteAiResponse};
use crate::notes::{self, NoteEntry};
use crate::pty_manager::PtyManager;
use crate::state::AppState;
use crate::storage::{ProjectInventory, Storage};
use crate::terminal_theme;
use crate::voice::VoicePipeline;
use crate::worktree::WorktreeManager;

// ---------------------------------------------------------------------------
// Helper functions (moved from commands.rs)
// ---------------------------------------------------------------------------

const DEFAULT_AGENTS_MD: &str = r#"# {name}

One-line project description.

## Task Structure (CRITICAL -- NEVER SKIP)

**This is the most important rule. Every task, no matter how small, MUST follow this structure before writing any code. No exceptions.**

1. **Definition**: What's the task? Why are we doing it? How will we approach it?
2. **Constraints**: What are the design constraints -- from the user prompt, codebase conventions, or what can be inferred?
3. **Validation**: How do I know for sure it was implemented as expected? Can I enforce it with flexible and non-brittle tests? I must validate before I consider a task complete. For semantic changes (bug fixes, feature refinements): if I revert my implementation, the test must still fail. After the implementation, the test must pass.

**If you catch yourself writing code without having stated all three above, STOP and state them first.**

## Key Docs

- `docs/plans/` -- Design and implementation plans.

## Tech Stack

<!-- Fill in your project's tech stack -->

## Dev Commands

<!-- Fill in your project's dev commands -->
"#;

/// Generate default `agents.md` content for a project.
pub fn render_agents_md(name: &str) -> String {
    DEFAULT_AGENTS_MD.replace("{name}", name)
}

/// Validate a project name. Rejects empty names, names containing `/` or `\`,
/// and names starting with `.`.
pub fn validate_project_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return Err(format!("Invalid project name: {}", name));
    }
    Ok(())
}

/// Create a `CLAUDE.md` symlink pointing to `agents.md` in the given directory,
/// if `agents.md` exists and `CLAUDE.md` does not.
pub fn ensure_claude_md_symlink(dir: &Path) -> Result<(), String> {
    let claude_md = dir.join("CLAUDE.md");
    let agents_md = dir.join("agents.md");
    if agents_md.exists() && !claude_md.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink("agents.md", &claude_md)
            .map_err(|e| format!("failed to create CLAUDE.md symlink: {}", e))?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file("agents.md", &claude_md)
            .map_err(|e| format!("failed to create CLAUDE.md symlink: {}", e))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Service functions
// ---------------------------------------------------------------------------

pub fn list_projects(state: &AppState) -> Result<ProjectInventory, AppError> {
    tracing::debug!("listing projects");
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let inventory = storage.list_projects().map_err(AppError::internal)?;
    Ok(inventory)
}

pub fn check_onboarding(state: &AppState) -> Result<Option<config::Config>, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let base_dir = storage.base_dir();
    Ok(config::load_config(&base_dir))
}

pub fn create_project(state: &AppState, name: &str, repo_path: &str) -> Result<Project, AppError> {
    tracing::info!(project_name = %name, repo_path = %repo_path, "creating project");
    validate_project_name(name).map_err(AppError::BadRequest)?;

    let path = Path::new(repo_path);
    if !path.is_dir() {
        tracing::error!(repo_path = %repo_path, "create_project: repo_path is not a directory");
        return Err(AppError::BadRequest(format!(
            "repo_path is not a directory: {}",
            repo_path
        )));
    }

    let storage = state.storage.lock().map_err(AppError::internal)?;

    // Reject duplicate project names.
    if let Ok(inventory) = storage.list_projects() {
        let existing = inventory.projects;
        if existing.iter().any(|p| p.name == name) {
            tracing::warn!(project_name = %name, "create_project: duplicate project name");
            return Err(AppError::BadRequest(format!(
                "A project named '{}' already exists",
                name
            )));
        }
    }

    let project = Project {
        id: Uuid::new_v4(),
        name: name.to_string(),
        repo_path: repo_path.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        archived: false,
        maintainer: crate::models::MaintainerConfig::default(),
        auto_worker: crate::models::AutoWorkerConfig::default(),
        prompts: vec![],
        sessions: vec![],
        staged_sessions: vec![],
    };

    storage.save_project(&project).map_err(AppError::internal)?;

    // If repo doesn't have agents.md, create default one in config dir
    let repo_agents = path.join("agents.md");
    if !repo_agents.exists() {
        storage
            .save_agents_md(project.id, &render_agents_md(&project.name))
            .map_err(AppError::internal)?;
    }

    // If repo has agents.md but no CLAUDE.md, create symlink
    ensure_claude_md_symlink(path).map_err(AppError::Internal)?;

    Ok(project)
}

pub fn load_project(state: &AppState, name: &str, repo_path: &str) -> Result<Project, AppError> {
    tracing::info!(project_name = %name, repo_path = %repo_path, "loading project");
    validate_project_name(name).map_err(AppError::BadRequest)?;

    let path = Path::new(repo_path);
    if !path.is_dir() {
        tracing::error!(repo_path = %repo_path, "load_project: repo_path is not a directory");
        return Err(AppError::BadRequest(format!(
            "repo_path is not a directory: {}",
            repo_path
        )));
    }

    // Validate it's a git repo
    let git_dir = path.join(".git");
    if !git_dir.exists() {
        tracing::error!(repo_path = %repo_path, "load_project: not a git repository");
        return Err(AppError::BadRequest(format!(
            "not a git repository: {}",
            repo_path
        )));
    }

    let storage = state.storage.lock().map_err(AppError::internal)?;

    // Return existing project if one with the same repo_path exists
    if let Ok(inventory) = storage.list_projects() {
        let existing = inventory.projects;
        if let Some(project) = existing.iter().find(|p| p.repo_path == repo_path) {
            return Ok(project.clone());
        }
        // Reject duplicate project names when creating new.
        if existing.iter().any(|p| p.name == name) {
            return Err(AppError::BadRequest(format!(
                "A project named '{}' already exists",
                name
            )));
        }
    }

    let project = Project {
        id: Uuid::new_v4(),
        name: name.to_string(),
        repo_path: repo_path.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        archived: false,
        maintainer: crate::models::MaintainerConfig::default(),
        auto_worker: crate::models::AutoWorkerConfig::default(),
        prompts: vec![],
        sessions: vec![],
        staged_sessions: vec![],
    };

    storage.save_project(&project).map_err(AppError::internal)?;

    // Only create default agents.md if repo doesn't have one
    let repo_agents = path.join("agents.md");
    if !repo_agents.exists() {
        storage
            .save_agents_md(project.id, &render_agents_md(&project.name))
            .map_err(AppError::internal)?;
    }

    // If repo has agents.md but no CLAUDE.md, create symlink
    ensure_claude_md_symlink(path).map_err(AppError::Internal)?;

    Ok(project)
}

/// Delete a project. This is synchronous — callers that need non-blocking
/// behaviour (e.g. the Tauri command) should wrap in `spawn_blocking`.
///
/// Takes the individual `Arc` fields instead of `&AppState` so that callers
/// can clone them before entering a `spawn_blocking` closure.
pub fn delete_project(
    storage: &Arc<Mutex<Storage>>,
    pty_manager: &Arc<Mutex<PtyManager>>,
    project_id: Uuid,
    delete_repo: bool,
) -> Result<(), AppError> {
    tracing::info!(project_id = %project_id, delete_repo, "deleting project");

    let storage = storage.lock().map_err(AppError::internal)?;
    let project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;

    // Close all PTY sessions and clean up worktrees
    {
        let mut pty_manager = pty_manager.lock().map_err(AppError::internal)?;
        for session in &project.sessions {
            let _ = pty_manager.close_session(session.id);
            if let (Some(wt_path), Some(branch)) =
                (&session.worktree_path, &session.worktree_branch)
            {
                let _ = WorktreeManager::remove_worktree(wt_path, &project.repo_path, branch);
            }
        }
    }

    // Delete project metadata from ~/.the-controller/projects/{id}/
    storage
        .delete_project_dir(project_id)
        .map_err(AppError::internal)?;

    // Optionally delete the repo directory
    if delete_repo && Path::new(&project.repo_path).exists() {
        std::fs::remove_dir_all(&project.repo_path)
            .map_err(|e| AppError::Internal(format!("failed to delete repo: {}", e)))?;
    }

    Ok(())
}

pub fn get_agents_md(state: &AppState, project_id: Uuid) -> Result<String, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;
    storage.get_agents_md(&project).map_err(AppError::internal)
}

pub fn update_agents_md(state: &AppState, project_id: Uuid, content: &str) -> Result<(), AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    storage
        .save_agents_md(project_id, content)
        .map_err(AppError::internal)
}

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
pub fn connect_session(
    state: &AppState,
    session_id: Uuid,
    rows: u16,
    cols: u16,
) -> Result<(), AppError> {
    // Check if already connected
    {
        let pty_manager = state.pty_manager.lock().map_err(AppError::internal)?;
        if pty_manager.sessions.contains_key(&session_id) {
            tracing::debug!(session_id = %session_id, "session already connected, skipping");
            return Ok(());
        }
    }
    tracing::info!(session_id = %session_id, rows, cols, "connecting session to PTY");

    // Find session config from storage
    let (session_dir, kind) = {
        let storage = state.storage.lock().map_err(AppError::internal)?;
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

    let mut mgr = state.pty_manager.lock().map_err(AppError::internal)?;
    mgr.spawn_session(
        session_id,
        &session_dir,
        &kind,
        state.emitter.clone(),
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
) -> Result<u16, String> {
    use std::process::Stdio;

    tracing::info!(session_id = %session_id, project_id = %project_id, "staging session");

    let _staging_guard = state.staging_lock.lock().await;

    // Extract data under a short-lived storage lock to avoid deadlock with pty_manager
    let (repo_path, branch, worktree_path) = {
        let storage = state.storage.lock().map_err(|e| e.to_string())?;
        let project = storage
            .load_project(project_id)
            .map_err(|e| e.to_string())?;

        if project.name != "the-controller" {
            tracing::warn!(project_name = %project.name, "staging rejected: only supported for the-controller");
            return Err("Staging is only supported for the-controller".to_string());
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
                return Err("This session is already staged — unstage it first".to_string());
            }
            // Stale record — clean up
            kill_process_group(existing.pid);
            let stale_socket = crate::status_socket::staged_socket_path(&session_id);
            let _ = std::fs::remove_file(&stale_socket);
            let mut p = project.clone();
            p.staged_sessions.retain(|s| s.session_id != session_id);
            storage.save_project(&p).map_err(|e| e.to_string())?;
        }

        let session = project
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .ok_or("Session not found")?;

        let branch = session
            .worktree_branch
            .as_deref()
            .ok_or("Session has no worktree branch")?
            .to_string();

        let worktree_path = session
            .worktree_path
            .as_deref()
            .ok_or("Session has no worktree path")?
            .to_string();

        (project.repo_path.clone(), branch, worktree_path)
    };

    // 1. Ensure worktree is clean
    {
        let wt = worktree_path.clone();
        let is_clean = tokio::task::spawn_blocking(move || WorktreeManager::is_worktree_clean(&wt))
            .await
            .map_err(|e| format!("Task failed: {}", e))??;

        if !is_clean {
            if !allow_pty_prompts {
                return Err("Worktree has uncommitted changes — commit before staging".to_string());
            }

            let prompt = "\nYou have uncommitted changes. Please commit all your work now.\r";
            {
                let mut pty_manager = state.pty_manager.lock().map_err(|e| e.to_string())?;
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
                .map_err(|e| format!("Task failed: {}", e))??;
                if clean {
                    committed = true;
                    break;
                }
            }
            if !committed {
                tracing::error!(session_id = %session_id, "stage_session: timed out waiting for commit");
                return Err(
                    "Timed out waiting for commit. Please commit manually and retry.".to_string(),
                );
            }
        }
    }

    // 2. Rebase onto main if needed
    {
        let rp = repo_path.clone();
        let main_branch =
            tokio::task::spawn_blocking(move || WorktreeManager::detect_main_branch(&rp))
                .await
                .map_err(|e| format!("Task failed: {}", e))??;

        let rp = repo_path.clone();
        let mb2 = main_branch.clone();
        let _ = tokio::task::spawn_blocking(move || WorktreeManager::sync_main(&rp, &mb2)).await;

        let rp = repo_path.clone();
        let br = branch.clone();
        let mb = main_branch.clone();
        let is_behind =
            tokio::task::spawn_blocking(move || WorktreeManager::is_branch_behind(&rp, &br, &mb))
                .await
                .map_err(|e| format!("Task failed: {}", e))??;

        if is_behind {
            let wt = worktree_path.clone();
            let mb = main_branch.clone();
            let rebase_clean =
                tokio::task::spawn_blocking(move || WorktreeManager::rebase_onto(&wt, &mb))
                    .await
                    .map_err(|e| format!("Task failed: {}", e))??;

            if !rebase_clean {
                if !allow_pty_prompts {
                    return Err("Rebase has conflicts — resolve before staging".to_string());
                }

                // Rebase has conflicts — ask Claude to resolve
                let prompt = "\nThere are rebase conflicts. Please resolve all conflicts, then run `git rebase --continue`.\r";
                {
                    let mut pty_manager = state.pty_manager.lock().map_err(|e| e.to_string())?;
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
                    .map_err(|e| format!("Task failed: {}", e))?;
                    if !still_rebasing {
                        resolved = true;
                        break;
                    }
                }
                if !resolved {
                    tracing::error!(session_id = %session_id, "stage_session: timed out waiting for rebase conflict resolution");
                    return Err("Timed out waiting for rebase conflict resolution.".to_string());
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
        .map_err(|e| format!("Task failed: {}", e))?
        .map_err(|e| format!("pnpm install failed: {}", e))?;

        if !install_status.success() {
            return Err("pnpm install failed in worktree".to_string());
        }
    }

    let port = find_staging_port(1420)?;

    let _ = state
        .emitter
        .emit("staging-status", &format!("Starting on port {}...", port));

    let wt = worktree_path.clone();
    let log_path = PathBuf::from(&wt).join("staging.log");
    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| format!("Failed to create staging log: {}", e))?;
    let log_stderr = log_file
        .try_clone()
        .map_err(|e| format!("Failed to clone log file: {}", e))?;

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
            .map_err(|e| format!("Failed to spawn staged instance: {}", e))?
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
        .map_err(|e| format!("Failed to spawn staged instance: {}", e))?;

    let pid = child.id();
    tracing::info!(session_id = %session_id, pid, port, "staged instance spawned");
    // Reap the child in a background thread to prevent zombie entries.
    // We manage the process lifetime via PID/process group (kill_process_group),
    // not via this Child handle.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    // Save staged session info — if save fails, kill the orphan process
    let save_result = (|| -> Result<(), String> {
        let storage = state.storage.lock().map_err(|e| e.to_string())?;
        let mut project = storage
            .load_project(project_id)
            .map_err(|e| e.to_string())?;

        project.staged_sessions.push(StagedSession {
            session_id,
            pid,
            port,
        });

        storage.save_project(&project).map_err(|e| e.to_string())
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
// Notes service functions
// ---------------------------------------------------------------------------

/// Best-effort git commit for notes. Logs errors but never fails the caller.
pub fn try_commit_notes(base_dir: &Path, message: &str) {
    tracing::debug!("committing notes");
    if let Err(e) = notes::commit_notes(base_dir, message) {
        tracing::error!(error = %e, "notes git commit failed");
    }
}

pub fn list_notes(storage: &Arc<Mutex<Storage>>, folder: &str) -> Result<Vec<NoteEntry>, AppError> {
    tracing::debug!("listing notes");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::list_notes(&base_dir, folder).map_err(AppError::internal)
}

pub fn read_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    filename: &str,
) -> Result<String, AppError> {
    tracing::debug!("reading note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::read_note(&base_dir, folder, filename).map_err(AppError::internal)
}

pub fn write_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    filename: &str,
    content: &str,
) -> Result<(), AppError> {
    tracing::debug!("writing note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::write_note(&base_dir, folder, filename, content).map_err(AppError::internal)
    // No git commit here — batched via commit_notes command
}

pub fn create_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    title: &str,
) -> Result<String, AppError> {
    tracing::debug!("creating note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    let filename = notes::create_note(&base_dir, folder, title).map_err(AppError::internal)?;
    try_commit_notes(&base_dir, &format!("create {}/{}", folder, filename));
    Ok(filename)
}

pub fn delete_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    filename: &str,
) -> Result<(), AppError> {
    tracing::debug!("deleting note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::delete_note(&base_dir, folder, filename).map_err(AppError::internal)?;
    try_commit_notes(&base_dir, &format!("delete {}/{}", folder, filename));
    Ok(())
}

pub fn rename_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    old_name: &str,
    new_name: &str,
) -> Result<String, AppError> {
    tracing::debug!("renaming note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    let new_filename =
        notes::rename_note(&base_dir, folder, old_name, new_name).map_err(AppError::internal)?;
    try_commit_notes(
        &base_dir,
        &format!("rename {}/{} → {}", folder, old_name, new_filename),
    );
    Ok(new_filename)
}

pub fn duplicate_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    filename: &str,
) -> Result<String, AppError> {
    tracing::debug!("duplicating note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    let copy = notes::duplicate_note(&base_dir, folder, filename).map_err(AppError::internal)?;
    try_commit_notes(
        &base_dir,
        &format!("duplicate {}/{} → {}", folder, filename, copy),
    );
    Ok(copy)
}

pub fn list_note_folders(storage: &Arc<Mutex<Storage>>) -> Result<Vec<String>, AppError> {
    tracing::debug!("listing note folders");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::list_folders(&base_dir).map_err(AppError::internal)
}

pub fn create_note_folder(storage: &Arc<Mutex<Storage>>, name: &str) -> Result<(), AppError> {
    tracing::debug!("creating folder");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::create_folder(&base_dir, name).map_err(AppError::internal)?;
    try_commit_notes(&base_dir, &format!("create folder {}", name));
    Ok(())
}

pub fn rename_note_folder(
    storage: &Arc<Mutex<Storage>>,
    old_name: &str,
    new_name: &str,
) -> Result<(), AppError> {
    tracing::debug!("renaming folder");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::rename_folder(&base_dir, old_name, new_name).map_err(AppError::internal)?;
    try_commit_notes(
        &base_dir,
        &format!("rename folder {} → {}", old_name, new_name),
    );
    Ok(())
}

pub fn delete_note_folder(
    storage: &Arc<Mutex<Storage>>,
    name: &str,
    force: bool,
) -> Result<(), AppError> {
    tracing::debug!(force, "deleting folder");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::delete_folder(&base_dir, name, force).map_err(AppError::internal)?;
    try_commit_notes(&base_dir, &format!("delete folder {}", name));
    Ok(())
}

/// Commit any pending note changes (content edits).
/// Called by the frontend when switching notes.
pub fn commit_pending_notes(storage: &Arc<Mutex<Storage>>) -> Result<bool, AppError> {
    tracing::debug!("committing pending note changes");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::commit_notes(&base_dir, "update notes").map_err(AppError::internal)
}

pub fn save_note_image(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    image_bytes: &[u8],
    extension: &str,
) -> Result<String, AppError> {
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::save_note_image(&base_dir, folder, image_bytes, extension).map_err(AppError::internal)
}

pub fn resolve_note_asset_path(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    relative_path: &str,
) -> Result<String, AppError> {
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::resolve_note_asset_path(&base_dir, folder, relative_path)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(AppError::internal)
}

pub async fn send_note_ai_chat(
    note_content: String,
    selected_text: String,
    conversation_history: Vec<NoteAiChatMessage>,
    prompt: String,
) -> Result<NoteAiResponse, AppError> {
    crate::note_ai_chat::send_note_ai_message(
        std::env::temp_dir().to_string_lossy().to_string(),
        note_content,
        selected_text,
        conversation_history,
        prompt,
    )
    .await
    .map_err(AppError::Internal)
}

// ---------------------------------------------------------------------------
// GitHub issue commands
// ---------------------------------------------------------------------------

/// Worker report parsed from a closed GitHub issue with the `assigned-to-auto-worker` label.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkerReport {
    pub issue_number: u64,
    pub title: String,
    pub comment_body: String,
    pub updated_at: String,
}

const WORKER_REPORT_FALLBACK_BODY: &str = "No worker report was posted for this issue.";
const LABEL_ASSIGNED_TO_AUTO_WORKER: &str = "assigned-to-auto-worker";

/// Parse a GitHub remote URL into an "owner/repo" string.
/// Handles SSH (git@github.com:owner/repo.git), HTTPS, and HTTP URLs.
pub fn parse_github_nwo(url: &str) -> Result<String, String> {
    // SSH: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return Ok(rest.trim_end_matches(".git").to_string());
    }
    // HTTPS/HTTP: https://github.com/owner/repo.git
    if let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        return Ok(rest.trim_end_matches(".git").to_string());
    }

    Err(format!("Not a GitHub remote URL: {}", url))
}

/// Parse a GitHub issue URL like "https://github.com/owner/repo/issues/42" and return the issue number.
fn parse_github_issue_url(url: &str) -> Result<u64, String> {
    let url = url.trim();
    let parts: Vec<&str> = url.rsplitn(2, '/').collect();
    if parts.len() == 2 {
        if let Ok(num) = parts[0].parse::<u64>() {
            return Ok(num);
        }
    }
    Err(format!("Could not parse issue number from URL: {}", url))
}

/// Extract the GitHub owner/repo from a local git repository's origin remote.
/// Uses `spawn_blocking` because git2 operations block the thread.
pub async fn extract_github_repo(repo_path: &str) -> Result<String, AppError> {
    let rp = repo_path.to_string();
    tokio::task::spawn_blocking(move || {
        let repo =
            git2::Repository::discover(&rp).map_err(|e| format!("Failed to open repo: {}", e))?;
        let remote = repo
            .find_remote("origin")
            .map_err(|_| "No 'origin' remote found".to_string())?;
        let url = remote
            .url()
            .ok_or_else(|| "Origin remote URL is not valid UTF-8".to_string())?;
        parse_github_nwo(url)
    })
    .await
    .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
    .map_err(AppError::Internal)
}

/// Fetch open GitHub issues for a repo via the `gh` CLI.
pub async fn fetch_github_issues(repo_path: &str) -> Result<Vec<GithubIssue>, AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    tracing::debug!(repo = %nwo, "fetching issues via gh issue list");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            &nwo,
            "--json",
            "number,title,url,body,labels",
            "--limit",
            "50",
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, error = %e, "failed to spawn gh process");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("rate limit") || stderr.contains("403") {
            tracing::warn!(repo = %nwo, "GitHub API rate limit detected");
        }
        tracing::error!(repo = %nwo, stderr = %stderr, "gh issue list failed");
        return Err(AppError::Internal(format!(
            "gh issue list failed: {}",
            stderr
        )));
    }

    let issues: Vec<GithubIssue> = serde_json::from_slice(&output.stdout).map_err(|e| {
        tracing::error!(repo = %nwo, error = %e, "failed to parse gh issue list output");
        AppError::Internal(format!("Failed to parse gh output: {}", e))
    })?;

    tracing::debug!(repo = %nwo, count = issues.len(), "fetched issues");
    Ok(issues)
}

/// List GitHub issues with caching (stale-while-revalidate).
pub async fn list_github_issues(
    state: &AppState,
    repo_path: &str,
) -> Result<Vec<GithubIssue>, AppError> {
    // Check cache (lock is dropped at end of block before any .await)
    let cache_result = {
        let cache = state
            .issue_cache
            .lock()
            .map_err(|e| AppError::Internal(format!("Cache lock error: {}", e)))?;
        match cache.get(repo_path) {
            Some(entry) if entry.is_fresh() => {
                tracing::debug!(repo = %repo_path, "issue cache hit (fresh)");
                return Ok(entry.issues.clone());
            }
            Some(entry) => {
                tracing::debug!(repo = %repo_path, "issue cache hit (stale), refreshing in background");
                Some(entry.issues.clone())
            }
            None => {
                tracing::debug!(repo = %repo_path, "issue cache miss");
                None
            }
        }
    };

    if let Some(stale_issues) = cache_result {
        // Spawn background refresh
        let cache_arc = state.issue_cache.clone();
        let repo_path_bg = repo_path.to_string();
        tokio::spawn(async move {
            if let Ok(fresh_issues) = fetch_github_issues(&repo_path_bg).await {
                if let Ok(mut cache) = cache_arc.lock() {
                    cache.insert(repo_path_bg, fresh_issues);
                }
            }
        });
        return Ok(stale_issues);
    }

    // Cache miss: fetch, cache, and return
    let issues = fetch_github_issues(repo_path).await?;
    {
        let mut cache = state
            .issue_cache
            .lock()
            .map_err(|e| AppError::Internal(format!("Cache lock error: {}", e)))?;
        cache.insert(repo_path.to_string(), issues.clone());
    }
    Ok(issues)
}

/// Generate a GitHub issue body using the Claude CLI.
pub async fn generate_issue_body(title: &str) -> Result<String, AppError> {
    tracing::debug!("generating issue body via claude CLI");
    let prompt = format!(
        "Write a concise GitHub issue body for an issue titled: \"{}\". \
         Include a Summary section and a Details section. \
         Keep it under 200 words. Return only the markdown body, nothing else.",
        title
    );
    let output = tokio::process::Command::new("claude")
        .args(["--print", &prompt])
        .env_remove("CLAUDECODE")
        .output()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to spawn claude CLI");
            AppError::Internal(format!("Failed to run claude: {}", e))
        })?;

    if output.status.success() {
        tracing::debug!("claude CLI generated issue body");
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        tracing::warn!("claude CLI returned non-zero exit, using empty body");
        Ok(String::new())
    }
}

/// Create a new GitHub issue via the `gh` CLI.
pub async fn create_github_issue(
    state: &AppState,
    repo_path: &str,
    title: &str,
    body: &str,
) -> Result<GithubIssue, AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    tracing::info!(repo = %nwo, title = %title, "creating GitHub issue");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue", "create", "--repo", &nwo, "--title", title, "--body", body,
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, error = %e, "failed to spawn gh for issue create");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("rate limit") || stderr.contains("403") {
            tracing::warn!(repo = %nwo, "GitHub API rate limit detected during issue create");
        }
        tracing::error!(repo = %nwo, stderr = %stderr, "gh issue create failed");
        return Err(AppError::Internal(format!(
            "gh issue create failed: {}",
            stderr
        )));
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let number = parse_github_issue_url(&url).map_err(AppError::Internal)?;

    tracing::info!(repo = %nwo, issue_number = number, "created GitHub issue");

    let issue = GithubIssue {
        number,
        title: title.to_string(),
        url,
        body: Some(body.to_string()),
        labels: vec![],
    };

    if let Ok(mut cache) = state.issue_cache.lock() {
        cache.add_issue(repo_path, issue.clone());
    }

    Ok(issue)
}

/// Post a comment on a GitHub issue via the `gh` CLI.
pub async fn post_github_comment(
    repo_path: &str,
    issue_number: u64,
    body: &str,
) -> Result<(), AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    tracing::debug!(repo = %nwo, issue_number, "posting comment on issue");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "comment",
            &issue_number.to_string(),
            "--repo",
            &nwo,
            "--body",
            body,
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, issue_number, error = %e, "failed to spawn gh for comment");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(repo = %nwo, issue_number, stderr = %stderr, "gh issue comment failed");
        return Err(AppError::Internal(format!(
            "gh issue comment failed: {}",
            stderr
        )));
    }

    tracing::debug!(repo = %nwo, issue_number, "comment posted");
    Ok(())
}

/// Add a label to a GitHub issue (ensuring the label exists on the repo first).
pub async fn add_github_label(
    state: &AppState,
    repo_path: &str,
    issue_number: u64,
    label: &str,
    description: Option<&str>,
    color: Option<&str>,
) -> Result<(), AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    let desc = description.unwrap_or("Issue is being worked on in a session");
    let col = color.unwrap_or("F9E2AF");

    tracing::debug!(repo = %nwo, label = %label, "ensuring label exists on repo");
    // Ensure the label exists on the repo (ignore errors if it already exists)
    let _ = tokio::process::Command::new("gh")
        .args([
            "label",
            "create",
            label,
            "--repo",
            &nwo,
            "--description",
            desc,
            "--color",
            col,
        ])
        .output()
        .await;

    tracing::debug!(repo = %nwo, issue_number, label = %label, "adding label to issue");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "edit",
            &issue_number.to_string(),
            "--repo",
            &nwo,
            "--add-label",
            label,
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, issue_number, error = %e, "failed to spawn gh for add label");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(repo = %nwo, issue_number, label = %label, stderr = %stderr, "gh issue edit (add label) failed");
        return Err(AppError::Internal(format!(
            "gh issue edit failed: {}",
            stderr
        )));
    }

    tracing::debug!(repo = %nwo, issue_number, label = %label, "label added");
    if let Ok(mut cache) = state.issue_cache.lock() {
        cache.add_label(repo_path, issue_number, label);
    }

    Ok(())
}

/// Remove a label from a GitHub issue.
pub async fn remove_github_label(
    state: &AppState,
    repo_path: &str,
    issue_number: u64,
    label: &str,
) -> Result<(), AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    tracing::debug!(repo = %nwo, issue_number, label = %label, "removing label from issue");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "edit",
            &issue_number.to_string(),
            "--repo",
            &nwo,
            "--remove-label",
            label,
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, issue_number, error = %e, "failed to spawn gh for remove label");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(repo = %nwo, issue_number, label = %label, stderr = %stderr, "gh issue edit (remove label) failed");
        return Err(AppError::Internal(format!(
            "gh issue edit failed: {}",
            stderr
        )));
    }

    tracing::debug!(repo = %nwo, issue_number, label = %label, "label removed");
    if let Ok(mut cache) = state.issue_cache.lock() {
        cache.remove_label(repo_path, issue_number, label);
    }

    Ok(())
}

/// Close a GitHub issue, optionally with a closing comment.
pub async fn close_github_issue(
    state: &AppState,
    repo_path: &str,
    issue_number: u64,
    comment: &str,
) -> Result<(), AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    let mut args = vec![
        "issue".to_string(),
        "close".to_string(),
        issue_number.to_string(),
        "--repo".to_string(),
        nwo,
    ];

    if !comment.trim().is_empty() {
        args.push("--comment".to_string());
        args.push(comment.to_string());
    }

    let output = tokio::process::Command::new("gh")
        .args(&args)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to run gh: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(format!(
            "gh issue close failed: {}",
            stderr
        )));
    }

    // Remove from cache since list only shows open issues
    if let Ok(mut cache) = state.issue_cache.lock() {
        cache.remove_issue(repo_path, issue_number);
    }

    Ok(())
}

/// Delete a GitHub issue permanently.
pub async fn delete_github_issue(
    state: &AppState,
    repo_path: &str,
    issue_number: u64,
) -> Result<(), AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "delete",
            &issue_number.to_string(),
            "--repo",
            &nwo,
            "--yes",
        ])
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to run gh: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(format!(
            "gh issue delete failed: {}",
            stderr
        )));
    }

    if let Ok(mut cache) = state.issue_cache.lock() {
        cache.remove_issue(repo_path, issue_number);
    }

    Ok(())
}

/// Fetch issues labeled "filed-by-maintainer" for a repo.
pub async fn get_maintainer_issues(
    repo_path: &str,
    github_repo: Option<&str>,
) -> Result<Vec<MaintainerIssue>, AppError> {
    let nwo = match github_repo {
        Some(repo) if !repo.is_empty() => repo.to_string(),
        _ => extract_github_repo(repo_path).await?,
    };

    tracing::debug!(repo = %nwo, "fetching maintainer issues");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            &nwo,
            "--label",
            "filed-by-maintainer",
            "--state",
            "all",
            "--json",
            "number,title,state,url,labels,createdAt,closedAt",
            "--limit",
            "100",
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, error = %e, "failed to spawn gh for maintainer issues");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(repo = %nwo, stderr = %stderr, "gh issue list (maintainer) failed");
        return Err(AppError::Internal(format!(
            "gh issue list failed: {}",
            stderr
        )));
    }

    let issues: Vec<MaintainerIssue> = serde_json::from_slice(&output.stdout).map_err(|e| {
        tracing::error!(repo = %nwo, error = %e, "failed to parse maintainer issues");
        AppError::Internal(format!("Failed to parse gh output: {}", e))
    })?;

    tracing::debug!(repo = %nwo, count = issues.len(), "fetched maintainer issues");
    Ok(issues)
}

/// Fetch detailed information about a single maintainer issue.
pub async fn get_maintainer_issue_detail(
    repo_path: &str,
    github_repo: Option<&str>,
    issue_number: u32,
) -> Result<MaintainerIssueDetail, AppError> {
    let nwo = match github_repo {
        Some(repo) if !repo.is_empty() => repo.to_string(),
        _ => extract_github_repo(repo_path).await?,
    };

    tracing::debug!(repo = %nwo, issue_number, "fetching maintainer issue detail");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "view",
            &issue_number.to_string(),
            "--repo",
            &nwo,
            "--json",
            "number,title,state,body,url,labels,createdAt,closedAt",
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, issue_number, error = %e, "failed to spawn gh for issue detail");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(repo = %nwo, issue_number, stderr = %stderr, "gh issue view failed");
        return Err(AppError::Internal(format!(
            "gh issue view failed: {}",
            stderr
        )));
    }

    let detail: MaintainerIssueDetail = serde_json::from_slice(&output.stdout).map_err(|e| {
        tracing::error!(repo = %nwo, issue_number, error = %e, "failed to parse issue detail");
        AppError::Internal(format!("Failed to parse gh output: {}", e))
    })?;

    tracing::debug!(repo = %nwo, issue_number, "fetched maintainer issue detail");
    Ok(detail)
}

/// List issues that have at least one assignee.
pub async fn list_assigned_issues(repo_path: &str) -> Result<Vec<AssignedIssue>, AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    tracing::debug!(repo = %nwo, "fetching assigned issues");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            &nwo,
            "--json",
            "number,title,url,assignees,updatedAt,labels",
            "--limit",
            "100",
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, error = %e, "failed to spawn gh for assigned issues");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(repo = %nwo, stderr = %stderr, "gh issue list (assigned) failed");
        return Err(AppError::Internal(format!(
            "gh issue list failed: {}",
            stderr
        )));
    }

    let all_issues: Vec<AssignedIssue> = serde_json::from_slice(&output.stdout).map_err(|e| {
        tracing::error!(repo = %nwo, error = %e, "failed to parse assigned issues");
        AppError::Internal(format!("Failed to parse gh output: {}", e))
    })?;

    // Filter to only issues that have at least one assignee
    let assigned: Vec<AssignedIssue> = all_issues
        .into_iter()
        .filter(|issue| !issue.assignees.is_empty())
        .collect();

    tracing::debug!(repo = %nwo, count = assigned.len(), "fetched assigned issues");
    Ok(assigned)
}

/// Fetch worker reports (closed issues with the `assigned-to-auto-worker` label).
pub async fn get_worker_reports(repo_path: &str) -> Result<Vec<WorkerReport>, AppError> {
    let nwo = extract_github_repo(repo_path).await?;

    tracing::debug!(repo = %nwo, "fetching worker reports");
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            &nwo,
            "--label",
            LABEL_ASSIGNED_TO_AUTO_WORKER,
            "--state",
            "all",
            "--json",
            "number,title,state,comments,updatedAt",
            "--limit",
            "50",
        ])
        .output()
        .await
        .map_err(|e| {
            tracing::error!(repo = %nwo, error = %e, "failed to spawn gh for worker reports");
            AppError::Internal(format!("Failed to run gh: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(repo = %nwo, stderr = %stderr, "gh issue list (worker reports) failed");
        return Err(AppError::Internal(format!(
            "gh issue list failed: {}",
            stderr
        )));
    }

    let raw: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).map_err(|e| {
        tracing::error!(repo = %nwo, error = %e, "failed to parse worker reports");
        AppError::Internal(format!("Failed to parse gh output: {}", e))
    })?;

    let reports = parse_worker_reports(raw);
    tracing::debug!(repo = %nwo, count = reports.len(), "fetched worker reports");

    Ok(reports)
}

/// Parse raw JSON issue data into WorkerReport structs, filtering to closed issues.
pub fn parse_worker_reports(raw: Vec<serde_json::Value>) -> Vec<WorkerReport> {
    raw.into_iter()
        .filter_map(|issue| {
            if issue["state"].as_str() != Some("CLOSED") {
                return None;
            }
            let number = issue["number"].as_u64()?;
            let title = issue["title"].as_str()?.to_string();
            let updated_at = issue["updatedAt"].as_str().unwrap_or("").to_string();
            let body = issue["comments"]
                .as_array()
                .and_then(|comments| {
                    comments.iter().rev().find_map(|c| {
                        let text = c["body"].as_str()?;
                        text.contains("<!-- auto-worker-report -->").then_some(text)
                    })
                })
                .unwrap_or(WORKER_REPORT_FALLBACK_BODY)
                .to_string();
            Some(WorkerReport {
                issue_number: number,
                title,
                comment_body: body,
                updated_at,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Configuration commands
// ---------------------------------------------------------------------------

/// Return the user's home directory path.
pub fn home_dir() -> Result<String, AppError> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| AppError::Internal("Could not determine home directory".to_string()))
}

/// Check the Claude CLI installation and authentication status.
/// This spawns a subprocess and should be called from a blocking context.
pub fn check_claude_cli() -> String {
    config::check_claude_cli_status()
}

/// Save onboarding config with a projects root and optional default provider.
/// If `default_provider` is `None`, defaults to `ClaudeCode`.
pub fn save_onboarding_config(
    state: &AppState,
    projects_root: &str,
    default_provider: Option<config::ConfigDefaultProvider>,
) -> Result<(), AppError> {
    tracing::debug!(projects_root = %projects_root, "saving onboarding config");
    let path = Path::new(projects_root);
    if !path.is_dir() {
        tracing::error!(projects_root = %projects_root, "save_onboarding_config: not an existing directory");
        return Err(AppError::BadRequest(format!(
            "projects_root is not an existing directory: {}",
            projects_root
        )));
    }

    let storage = state.storage.lock().map_err(AppError::internal)?;
    let base_dir = storage.base_dir();

    // Preserve existing log_level to avoid clobbering it
    let existing_log_level = config::load_config(&base_dir)
        .map(|c| c.log_level)
        .unwrap_or_else(|| "info".to_string());

    let cfg = config::Config {
        projects_root: projects_root.to_string(),
        default_provider: default_provider.unwrap_or(config::ConfigDefaultProvider::ClaudeCode),
        log_level: existing_log_level,
    };
    config::save_config(&base_dir, &cfg).map_err(AppError::internal)
}

/// Load the terminal theme from the config directory.
/// This reads files and should be called from a blocking context.
pub fn load_terminal_theme_blocking(
    state: &AppState,
) -> Result<terminal_theme::TerminalTheme, AppError> {
    let base_dir = state.storage.lock().map_err(AppError::internal)?.base_dir();
    terminal_theme::load_terminal_theme(&base_dir).map_err(AppError::internal)
}

/// Load keybindings from the config directory.
pub fn load_keybindings(state: &AppState) -> Result<keybindings::KeybindingsResult, AppError> {
    let base_dir = state.storage.lock().map_err(AppError::internal)?.base_dir();
    Ok(keybindings::load_keybindings(&base_dir))
}

/// Log a frontend error to the dedicated log file and tracing.
pub fn log_frontend_error(state: &AppState, message: &str) {
    use std::io::Write;
    let sanitized = message.replace('\n', "\\n").replace('\r', "\\r");
    let timestamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z");
    let line = format!("{} ERROR [frontend] {}\n", timestamp, sanitized);

    if let Ok(mut guard) = state.frontend_log.lock() {
        if let Some(ref mut file) = *guard {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    tracing::error!(target: "frontend", "{}", sanitized);
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

// ---------------------------------------------------------------------------
// Deploy commands
// ---------------------------------------------------------------------------

/// Detect the project type based on files present in the repo.
/// Should be called from a blocking context.
pub fn detect_project_type_blocking(repo_path: &str) -> Result<ProjectSignals, AppError> {
    tracing::debug!(repo_path = %repo_path, "detecting project type");
    let path = Path::new(repo_path);
    let has_package_json = path.join("package.json").exists();
    let has_start_script = if has_package_json {
        std::fs::read_to_string(path.join("package.json"))
            .map(|content| content.contains("\"start\""))
            .unwrap_or(false)
    } else {
        false
    };

    Ok(ProjectSignals {
        has_dockerfile: path.join("Dockerfile").exists(),
        has_package_json,
        has_vite_config: path.join("vite.config.ts").exists()
            || path.join("vite.config.js").exists()
            || path.join("astro.config.mjs").exists()
            || path.join("next.config.js").exists()
            || path.join("next.config.mjs").exists(),
        has_start_script,
        has_pyproject: path.join("pyproject.toml").exists()
            || path.join("requirements.txt").exists(),
    })
}

/// Load deploy credentials from the credential store.
/// Should be called from a blocking context.
pub fn get_deploy_credentials_blocking() -> Result<DeployCredentials, AppError> {
    tracing::debug!("loading deploy credentials");
    DeployCredentials::load().map_err(AppError::Internal)
}

/// Save deploy credentials to the credential store.
/// Should be called from a blocking context.
pub fn save_deploy_credentials_blocking(credentials: DeployCredentials) -> Result<(), AppError> {
    tracing::info!("saving deploy credentials");
    credentials.save().map_err(AppError::Internal)
}

/// Check if deploy is provisioned (credentials are complete).
/// Should be called from a blocking context.
pub fn is_deploy_provisioned_blocking() -> Result<bool, AppError> {
    let creds = DeployCredentials::load().map_err(AppError::Internal)?;
    Ok(creds.is_provisioned())
}

/// Deploy a project via the Coolify API.
pub async fn deploy_project(request: DeployRequest) -> Result<DeployResult, AppError> {
    tracing::info!(
        project = %request.project_name,
        subdomain = %request.subdomain,
        project_type = %request.project_type,
        "starting project deployment"
    );
    let creds = tokio::task::spawn_blocking(DeployCredentials::load)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(AppError::Internal)?;
    if !creds.is_provisioned() {
        tracing::error!("deploy not provisioned — credentials incomplete");
        return Err(AppError::BadRequest(
            "Deploy not provisioned. Run setup first.".to_string(),
        ));
    }

    let coolify = CoolifyClient::new(
        creds.coolify_url.as_ref().unwrap(),
        creds.coolify_api_key.as_ref().unwrap(),
    );

    let apps = coolify
        .list_applications()
        .await
        .map_err(AppError::Internal)?;
    let existing = apps.iter().find(|a| a.name == request.project_name);

    let uuid = if let Some(app) = existing {
        tracing::info!(uuid = %app.uuid, "found existing Coolify app, redeploying");
        coolify
            .deploy_application(&app.uuid)
            .await
            .map_err(AppError::Internal)?;
        app.uuid.clone()
    } else {
        tracing::error!(project = %request.project_name, "no existing Coolify app found");
        return Err(AppError::Internal(
            "Creating new Coolify applications not yet implemented. Create the app in Coolify UI first.".to_string(),
        ));
    };

    let domain = format!("{}.{}", request.subdomain, creds.root_domain.unwrap());
    let url = format!("https://{domain}");

    tracing::info!(url = %url, uuid = %uuid, "deployment complete");
    Ok(DeployResult {
        url,
        coolify_uuid: uuid,
    })
}

/// List deployed services via the Coolify API.
pub async fn list_deployed_services() -> Result<Vec<serde_json::Value>, AppError> {
    tracing::debug!("listing deployed services");
    let creds = tokio::task::spawn_blocking(DeployCredentials::load)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(AppError::Internal)?;
    if !creds.is_provisioned() {
        tracing::warn!("credentials not provisioned, returning empty service list");
        return Ok(vec![]);
    }

    let coolify = CoolifyClient::new(
        creds.coolify_url.as_ref().unwrap(),
        creds.coolify_api_key.as_ref().unwrap(),
    );

    let apps = coolify
        .list_applications()
        .await
        .map_err(AppError::Internal)?;
    let result: Vec<serde_json::Value> = apps
        .iter()
        .map(|app| {
            serde_json::json!({
                "uuid": app.uuid,
                "name": app.name,
                "status": app.status,
                "fqdn": app.fqdn,
            })
        })
        .collect();

    Ok(result)
}

// ---------------------------------------------------------------------------
// Voice commands
// ---------------------------------------------------------------------------

/// Start the voice pipeline. If already running, re-emits the current state.
pub async fn start_voice_pipeline(state: &AppState) -> Result<(), AppError> {
    tracing::info!("starting voice pipeline");
    // Snapshot generation before init — if stop is called during init, this will change.
    let gen_before = state
        .voice_generation
        .load(std::sync::atomic::Ordering::SeqCst);
    // Brief lock to check if already running
    {
        let pipeline = state.voice_pipeline.lock().await;
        if let Some(p) = pipeline.as_ref() {
            // Pipeline already running — emit current state so a remounted
            // frontend component picks up the correct label immediately.
            tracing::debug!("voice pipeline already running, re-emitting state");
            let voice_state = if p.is_paused() { "paused" } else { "listening" };
            let payload = serde_json::json!({ "state": voice_state }).to_string();
            let _ = state.emitter.emit("voice-state-changed", &payload);
            return Ok(());
        }
    }
    // Release lock during init to avoid blocking stop_voice_pipeline
    let emitter = state.emitter.clone();
    let new_pipeline = VoicePipeline::start(emitter)
        .await
        .map_err(AppError::Internal)?;
    // Re-acquire lock to store the pipeline
    let mut pipeline = state.voice_pipeline.lock().await;
    let gen_after = state
        .voice_generation
        .load(std::sync::atomic::Ordering::SeqCst);
    if pipeline.is_some() || gen_before != gen_after {
        // Another start raced us, or stop was called during init — drop the pipeline
        return Ok(());
    }
    *pipeline = Some(new_pipeline);
    Ok(())
}

/// Stop the voice pipeline.
pub async fn stop_voice_pipeline(state: &AppState) -> Result<(), AppError> {
    tracing::info!("stopping voice pipeline");
    // Bump generation so any in-flight start_voice_pipeline knows to discard its result.
    state
        .voice_generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut pipeline = state.voice_pipeline.lock().await;
    if let Some(p) = pipeline.take() {
        // p.stop() calls thread::join which blocks — run on blocking thread pool
        tokio::task::spawn_blocking(move || {
            let mut p = p;
            p.stop();
        })
        .await
        .map_err(|e| AppError::Internal(format!("Failed to stop pipeline: {e}")))?;
    }
    Ok(())
}

/// Toggle voice pause state. Returns `true` if now paused.
pub async fn toggle_voice_pause(state: &AppState) -> Result<bool, AppError> {
    let pipeline = state.voice_pipeline.lock().await;
    match pipeline.as_ref() {
        Some(p) => {
            let paused = p.toggle_pause();
            // Emit state change immediately for responsive UI
            let voice_state = if paused { "paused" } else { "listening" };
            let payload = serde_json::json!({ "state": voice_state }).to_string();
            let _ = state.emitter.emit("voice-state-changed", &payload);
            Ok(paused)
        }
        None => Err(AppError::BadRequest(
            "Voice pipeline not running".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Auth/Login commands
// ---------------------------------------------------------------------------

/// Start a Claude login session by spawning a `claude login` PTY command.
/// Should be called from a blocking context.
pub fn start_claude_login(
    pty_manager: &Arc<Mutex<PtyManager>>,
    emitter: Arc<dyn EventEmitter>,
) -> Result<String, AppError> {
    tracing::info!("starting Claude login session");
    let session_id = Uuid::new_v4();
    let mut mgr = pty_manager.lock().map_err(AppError::internal)?;
    mgr.spawn_command(session_id, "claude", &["login"], emitter)
        .map_err(AppError::Internal)?;
    Ok(session_id.to_string())
}

/// Stop a Claude login session.
pub fn stop_claude_login(
    pty_manager: &Arc<Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<(), AppError> {
    let mut mgr = pty_manager.lock().map_err(AppError::internal)?;
    mgr.close_session(session_id).map_err(AppError::Internal)
}

#[cfg(test)]
mod github_tests {
    use super::*;

    #[test]
    fn test_parse_github_nwo_ssh() {
        assert_eq!(
            parse_github_nwo("git@github.com:owner/repo.git").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn test_parse_github_nwo_https() {
        assert_eq!(
            parse_github_nwo("https://github.com/owner/repo.git").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn test_parse_github_nwo_https_no_git_suffix() {
        assert_eq!(
            parse_github_nwo("https://github.com/owner/repo").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn test_parse_github_nwo_http() {
        assert_eq!(
            parse_github_nwo("http://github.com/owner/repo.git").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn test_parse_github_nwo_non_github_url() {
        let result = parse_github_nwo("https://gitlab.com/owner/repo.git");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Not a GitHub remote URL"));
    }

    #[test]
    fn test_parse_github_issue_url_basic() {
        assert_eq!(
            parse_github_issue_url("https://github.com/owner/repo/issues/42").unwrap(),
            42
        );
    }

    #[test]
    fn test_parse_github_issue_url_trailing_newline() {
        assert_eq!(
            parse_github_issue_url("https://github.com/owner/repo/issues/7\n").unwrap(),
            7
        );
    }

    #[test]
    fn test_parse_github_issue_url_invalid() {
        assert!(parse_github_issue_url("not a url").is_err());
    }

    #[test]
    fn test_parse_github_nwo_ssh_no_git_suffix() {
        assert_eq!(
            parse_github_nwo("git@github.com:owner/repo").unwrap(),
            "owner/repo"
        );
    }

    #[test]
    fn test_parse_github_nwo_empty_string() {
        assert!(parse_github_nwo("").is_err());
    }

    #[test]
    fn test_parse_github_issue_url_large_number() {
        assert_eq!(
            parse_github_issue_url("https://github.com/owner/repo/issues/99999").unwrap(),
            99999
        );
    }

    #[test]
    fn test_parse_github_issue_url_zero() {
        assert_eq!(
            parse_github_issue_url("https://github.com/owner/repo/issues/0").unwrap(),
            0
        );
    }

    #[test]
    fn test_parse_github_issue_url_empty() {
        assert!(parse_github_issue_url("").is_err());
    }

    #[test]
    fn parse_worker_reports_excludes_open_issues() {
        let reports = parse_worker_reports(vec![
            serde_json::json!({
                "number": 42,
                "title": "Closed worker issue",
                "state": "CLOSED",
                "updatedAt": "2026-03-10T00:00:00Z",
                "comments": [],
            }),
            serde_json::json!({
                "number": 43,
                "title": "Open worker issue",
                "state": "OPEN",
                "updatedAt": "2026-03-10T00:00:00Z",
                "comments": [],
            }),
        ]);

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].issue_number, 42);
    }

    #[test]
    fn parse_worker_reports_uses_fallback_body_when_report_comment_missing() {
        let reports = parse_worker_reports(vec![serde_json::json!({
            "number": 42,
            "title": "Closed worker issue",
            "state": "CLOSED",
            "updatedAt": "2026-03-10T00:00:00Z",
            "comments": [],
        })]);

        assert_eq!(reports[0].comment_body, WORKER_REPORT_FALLBACK_BODY);
    }
}
