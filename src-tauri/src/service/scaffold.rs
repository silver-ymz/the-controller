use std::path::{Path, PathBuf};

use the_controller_macros::derive_handlers;

use crate::config;
use crate::error::AppError;
use crate::models::Project;
use crate::state::AppState;

fn github_cli_command() -> std::process::Command {
    std::process::Command::new(
        std::env::var("THE_CONTROLLER_GH_BIN").unwrap_or_else(|_| "gh".to_string()),
    )
}

fn git_cli_command() -> std::process::Command {
    std::process::Command::new(
        std::env::var("THE_CONTROLLER_GIT_BIN").unwrap_or_else(|_| "git".to_string()),
    )
}

fn rollback_scaffold_dir(repo_path: &Path, error: String) -> String {
    match std::fs::remove_dir_all(repo_path) {
        Ok(_) => error,
        Err(cleanup_error) => format!("{} (cleanup failed: {})", error, cleanup_error),
    }
}

pub(crate) fn rollback_scaffold_state(repo_path: &Path, error: String) -> String {
    let mut cleanup_errors = Vec::new();

    if let Ok(repo) = git2::Repository::open(repo_path) {
        if let Ok(remote) = repo.find_remote("origin") {
            if let Some(url) = remote.url() {
                if let Ok(nwo) = super::parse_github_nwo(url) {
                    match github_cli_command()
                        .args(["repo", "delete", &nwo, "--yes"])
                        .output()
                    {
                        Ok(output) if output.status.success() => {}
                        Ok(output) => cleanup_errors.push(format!(
                            "remote cleanup failed: {}",
                            String::from_utf8_lossy(&output.stderr).trim()
                        )),
                        Err(e) => cleanup_errors.push(format!("remote cleanup failed: {}", e)),
                    }
                }
            }
        }
    }

    if let Err(cleanup_error) = std::fs::remove_dir_all(repo_path) {
        cleanup_errors.push(format!("local cleanup failed: {}", cleanup_error));
    }

    if cleanup_errors.is_empty() {
        error
    } else {
        format!("{} ({})", error, cleanup_errors.join("; "))
    }
}

/// Scaffold a new project on disk: create repo, init git, create GitHub remote, push.
/// This is blocking — callers should wrap in `spawn_blocking`.
pub fn scaffold_project_blocking(name: String, repo_path: PathBuf) -> Result<Project, String> {
    tracing::info!(project_name = %name, repo_path = %repo_path.display(), "scaffolding project on disk");
    let parent_dir = repo_path
        .parent()
        .ok_or_else(|| format!("Invalid repo path: {}", repo_path.display()))?;
    std::fs::create_dir_all(parent_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir(&repo_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!("Directory already exists: {}", name)
        } else {
            e.to_string()
        }
    })?;
    let rollback_dir = |error: String| rollback_scaffold_dir(&repo_path, error);

    let repo = git2::Repository::init(&repo_path).map_err(|e| rollback_dir(e.to_string()))?;
    let sig = repo
        .signature()
        .unwrap_or_else(|_| git2::Signature::now("The Controller", "noreply@controller").unwrap());

    let agents_content = super::render_agents_md(&name);
    std::fs::write(repo_path.join("agents.md"), &agents_content)
        .map_err(|e| rollback_dir(format!("failed to write agents.md: {}", e)))?;
    super::ensure_claude_md_symlink(&repo_path).map_err(rollback_dir)?;
    let plans_dir = repo_path.join("docs").join("plans");
    std::fs::create_dir_all(&plans_dir)
        .map_err(|e| rollback_dir(format!("failed to create docs/plans: {}", e)))?;
    std::fs::write(plans_dir.join(".gitkeep"), "")
        .map_err(|e| rollback_dir(format!("failed to write .gitkeep: {}", e)))?;

    let mut index = repo
        .index()
        .map_err(|e| rollback_dir(format!("failed to get index: {}", e)))?;
    index
        .add_path(std::path::Path::new("agents.md"))
        .map_err(|e| rollback_dir(format!("failed to add agents.md to index: {}", e)))?;
    index
        .add_path(std::path::Path::new("CLAUDE.md"))
        .map_err(|e| rollback_dir(format!("failed to add CLAUDE.md to index: {}", e)))?;
    index
        .add_path(std::path::Path::new("docs/plans/.gitkeep"))
        .map_err(|e| rollback_dir(format!("failed to add .gitkeep to index: {}", e)))?;
    index
        .write()
        .map_err(|e| rollback_dir(format!("failed to write index: {}", e)))?;
    let tree_id = index
        .write_tree()
        .map_err(|e| rollback_dir(format!("failed to write tree: {}", e)))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| rollback_dir(format!("failed to find tree: {}", e)))?;

    repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
        .map_err(|e| rollback_dir(format!("failed to create initial commit: {}", e)))?;

    let gh_output = github_cli_command()
        .args([
            "repo",
            "create",
            &name,
            "--private",
            "--source=.",
            "--remote=origin",
        ])
        .current_dir(&repo_path)
        .output()
        .map_err(|e| rollback_dir(format!("Failed to run gh CLI: {}. Is gh installed?", e)))?;
    if !gh_output.status.success() {
        let stderr = String::from_utf8_lossy(&gh_output.stderr);
        return Err(rollback_dir(format!(
            "Failed to create GitHub repo: {}",
            stderr.trim()
        )));
    }

    let push_output = git_cli_command()
        .args(["push", "--set-upstream", "origin", "HEAD"])
        .current_dir(&repo_path)
        .output()
        .map_err(|e| {
            rollback_scaffold_state(&repo_path, format!("Failed to run git push: {}", e))
        })?;
    if !push_output.status.success() {
        let stderr = String::from_utf8_lossy(&push_output.stderr);
        return Err(rollback_scaffold_state(
            &repo_path,
            format!("Failed to push initial commit: {}", stderr.trim()),
        ));
    }

    Ok(Project {
        id: uuid::Uuid::new_v4(),
        name,
        repo_path: repo_path.to_string_lossy().to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        archived: false,
        maintainer: crate::models::MaintainerConfig::default(),
        auto_worker: crate::models::AutoWorkerConfig::default(),
        prompts: vec![],
        sessions: vec![],
        staged_sessions: vec![],
    })
}

/// Full scaffold orchestration: validate, check duplicates, resolve repo path,
/// run blocking scaffold, and save the project to storage.
#[derive_handlers(tauri_command, axum_handler)]
pub async fn scaffold_project(state: &AppState, name: &str) -> Result<Project, AppError> {
    tracing::info!(project_name = %name, "scaffolding new project");
    super::validate_project_name(name).map_err(AppError::BadRequest)?;

    let repo_path = {
        let storage = state.storage.lock().map_err(AppError::internal)?;

        // Reject duplicate project names.
        if let Ok(inventory) = storage.list_projects() {
            if inventory.projects.iter().any(|p| p.name == name) {
                return Err(AppError::BadRequest(format!(
                    "A project named '{}' already exists",
                    name
                )));
            }
        }

        let cfg = config::load_config(&storage.base_dir()).ok_or_else(|| {
            AppError::BadRequest("No config found. Complete onboarding first.".to_string())
        })?;

        Path::new(&cfg.projects_root).join(name)
    };
    if repo_path.exists() {
        return Err(AppError::BadRequest(format!(
            "Directory already exists: {}",
            name
        )));
    }

    let name_owned = name.to_string();
    let project =
        tokio::task::spawn_blocking(move || scaffold_project_blocking(name_owned, repo_path))
            .await
            .map_err(|e| AppError::Internal(format!("Task failed: {}", e)))?
            .map_err(AppError::Internal)?;

    let storage = state.storage.lock().map_err(AppError::internal)?;
    if let Ok(inventory) = storage.list_projects() {
        if inventory.projects.iter().any(|p| p.name == project.name) {
            drop(storage);
            return Err(AppError::Internal(rollback_scaffold_state(
                Path::new(&project.repo_path),
                format!("A project named '{}' already exists", project.name),
            )));
        }
    }
    storage.save_project(&project).map_err(AppError::internal)?;

    Ok(project)
}
