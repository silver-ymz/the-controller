//! Service layer — shared business logic for Tauri commands and Axum handlers.
//!
//! Each public function in this module (and its submodules) encapsulates a
//! single unit of business logic. Both `commands.rs` (Tauri IPC) and
//! `server/` (Axum HTTP) delegate here, keeping the API surfaces thin.
//!
//! Errors are returned as [`crate::error::AppError`], which converts into
//! the appropriate response type for each API surface.

use std::path::Path;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::config;
use crate::error::AppError;
use crate::models::Project;
use crate::pty_manager::PtyManager;
use crate::state::AppState;
use crate::storage::{ProjectInventory, Storage};
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
