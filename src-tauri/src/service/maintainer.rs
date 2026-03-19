use uuid::Uuid;

use crate::error::AppError;
use crate::models::{AutoWorkerQueueIssue, GithubIssue, MaintainerRunLog, Project};
use crate::state::AppState;
use the_controller_macros::derive_handlers;

/// Validate that a maintainer interval is at least 5 minutes.
pub fn validate_maintainer_interval(minutes: u64) -> Result<(), AppError> {
    if minutes < 5 {
        return Err(AppError::BadRequest(
            "Interval must be at least 5 minutes".to_string(),
        ));
    }
    Ok(())
}

#[derive_handlers(tauri_command, axum_handler)]
pub fn configure_maintainer(
    state: &AppState,
    project_id: Uuid,
    enabled: bool,
    interval_minutes: u64,
    github_repo: Option<String>,
) -> Result<(), AppError> {
    validate_maintainer_interval(interval_minutes)?;
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let mut project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;
    project.maintainer.enabled = enabled;
    project.maintainer.interval_minutes = interval_minutes;
    project.maintainer.github_repo = github_repo;
    storage.save_project(&project).map_err(AppError::internal)?;
    Ok(())
}

#[derive_handlers(tauri_command, axum_handler)]
pub fn configure_auto_worker(
    state: &AppState,
    project_id: Uuid,
    enabled: bool,
) -> Result<(), AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let mut project = storage
        .load_project(project_id)
        .map_err(AppError::internal)?;
    project.auto_worker.enabled = enabled;
    storage.save_project(&project).map_err(AppError::internal)?;
    Ok(())
}

#[derive_handlers(tauri_command, axum_handler)]
pub fn get_maintainer_status(
    state: &AppState,
    project_id: Uuid,
) -> Result<Option<MaintainerRunLog>, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    storage
        .latest_maintainer_run_log(project_id)
        .map_err(AppError::internal)
}

/// Get the maintainer run log history for a project.
pub fn get_maintainer_history(
    state: &AppState,
    project_id: Uuid,
    limit: usize,
) -> Result<Vec<MaintainerRunLog>, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    storage
        .maintainer_run_log_history(project_id, limit)
        .map_err(AppError::internal)
}

#[derive_handlers(tauri_command, axum_handler)]
pub async fn trigger_maintainer_check(
    state: &AppState,
    project_id: Uuid,
) -> Result<MaintainerRunLog, AppError> {
    let (repo_path, github_repo) = {
        let storage = state.storage.lock().map_err(AppError::internal)?;
        let project = storage
            .load_project(project_id)
            .map_err(AppError::internal)?;
        (
            project.repo_path.clone(),
            project.maintainer.github_repo.clone(),
        )
    };

    let _ = state
        .emitter
        .emit(&format!("maintainer-status:{}", project_id), "running");

    let log = match tokio::task::spawn_blocking(move || {
        crate::maintainer::run_maintainer_check(&repo_path, project_id, github_repo.as_deref())
    })
    .await
    .map_err(|e| AppError::Internal(format!("Task failed: {e}")))?
    {
        Ok(log) => log,
        Err(e) => {
            let _ = state
                .emitter
                .emit(&format!("maintainer-status:{}", project_id), "error");
            let _ = state
                .emitter
                .emit(&format!("maintainer-error:{}", project_id), &e);
            return Err(AppError::Internal(e));
        }
    };

    {
        let storage = state.storage.lock().map_err(AppError::internal)?;
        storage
            .save_maintainer_run_log(&log)
            .map_err(AppError::internal)?;
    }

    let _ = state
        .emitter
        .emit(&format!("maintainer-status:{}", project_id), "idle");

    Ok(log)
}

#[derive_handlers(tauri_command, axum_handler)]
pub fn clear_maintainer_reports(state: &AppState, project_id: Uuid) -> Result<(), AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    storage
        .clear_maintainer_run_logs(project_id)
        .map_err(AppError::internal)?;
    let _ = state
        .emitter
        .emit(&format!("maintainer-status:{}", project_id), "idle");
    Ok(())
}

// ---------------------------------------------------------------------------
// Auto-worker queue
// ---------------------------------------------------------------------------

fn queue_issue_from_github(issue: GithubIssue, is_active: bool) -> AutoWorkerQueueIssue {
    AutoWorkerQueueIssue {
        number: issue.number,
        title: issue.title,
        url: issue.url,
        body: issue.body,
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        is_active,
    }
}

fn active_auto_worker_issue(project: &Project) -> Option<GithubIssue> {
    project
        .sessions
        .iter()
        .find(|session| session.auto_worker_session)
        .and_then(|session| session.github_issue.clone())
}

/// Build the auto-worker queue from a list of issues and the active issue.
pub fn build_auto_worker_queue(
    issues: Vec<GithubIssue>,
    active_issue: Option<GithubIssue>,
) -> Vec<AutoWorkerQueueIssue> {
    let active_issue_number = active_issue.as_ref().map(|issue| issue.number);
    let mut queue = Vec::new();

    if let Some(issue) = active_issue {
        queue.push(queue_issue_from_github(issue, true));
    }

    queue.extend(
        issues
            .into_iter()
            .filter(crate::auto_worker::is_eligible)
            .filter(|issue| Some(issue.number) != active_issue_number)
            .map(|issue| queue_issue_from_github(issue, false)),
    );

    queue
}

#[derive_handlers(tauri_command, axum_handler)]
pub async fn get_auto_worker_queue(
    state: &AppState,
    project_id: Uuid,
) -> Result<Vec<AutoWorkerQueueIssue>, AppError> {
    let project = {
        let storage = state.storage.lock().map_err(AppError::internal)?;
        storage
            .load_project(project_id)
            .map_err(AppError::internal)?
    };

    let active_issue = active_auto_worker_issue(&project);
    let issues = super::list_github_issues(state, &project.repo_path).await?;
    Ok(build_auto_worker_queue(issues, active_issue))
}
