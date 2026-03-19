use tauri::State;

use crate::models::{AssignedIssue, GithubIssue};
use crate::service;
use crate::state::AppState;

// Re-export WorkerReport so callers in commands.rs don't break.
pub use crate::service::WorkerReport;

pub(crate) async fn list_github_issues(
    repo_path: String,
    state: State<'_, AppState>,
) -> Result<Vec<GithubIssue>, String> {
    service::list_github_issues(&state, &repo_path)
        .await
        .map_err(Into::into)
}

pub(crate) async fn generate_issue_body(title: String) -> Result<String, String> {
    service::generate_issue_body(&title)
        .await
        .map_err(Into::into)
}

pub(crate) async fn create_github_issue(
    state: State<'_, AppState>,
    repo_path: String,
    title: String,
    body: String,
) -> Result<GithubIssue, String> {
    service::create_github_issue(&state, &repo_path, &title, &body)
        .await
        .map_err(Into::into)
}

pub(crate) async fn post_github_comment(
    repo_path: String,
    issue_number: u64,
    body: String,
) -> Result<(), String> {
    service::post_github_comment(&repo_path, issue_number, &body)
        .await
        .map_err(Into::into)
}

pub(crate) async fn add_github_label(
    state: State<'_, AppState>,
    repo_path: String,
    issue_number: u64,
    label: String,
    description: Option<String>,
    color: Option<String>,
) -> Result<(), String> {
    service::add_github_label(
        &state,
        &repo_path,
        issue_number,
        &label,
        description.as_deref(),
        color.as_deref(),
    )
    .await
    .map_err(Into::into)
}

pub(crate) async fn remove_github_label(
    state: State<'_, AppState>,
    repo_path: String,
    issue_number: u64,
    label: String,
) -> Result<(), String> {
    service::remove_github_label(&state, &repo_path, issue_number, &label)
        .await
        .map_err(Into::into)
}

pub(crate) async fn close_github_issue(
    state: State<'_, AppState>,
    repo_path: String,
    issue_number: u64,
    comment: String,
) -> Result<(), String> {
    service::close_github_issue(&state, &repo_path, issue_number, &comment)
        .await
        .map_err(Into::into)
}

pub(crate) async fn delete_github_issue(
    state: State<'_, AppState>,
    repo_path: String,
    issue_number: u64,
) -> Result<(), String> {
    service::delete_github_issue(&state, &repo_path, issue_number)
        .await
        .map_err(Into::into)
}

pub(crate) async fn list_assigned_issues(repo_path: String) -> Result<Vec<AssignedIssue>, String> {
    service::list_assigned_issues(&repo_path)
        .await
        .map_err(Into::into)
}

pub(crate) async fn get_worker_reports(repo_path: String) -> Result<Vec<WorkerReport>, String> {
    service::get_worker_reports(&repo_path)
        .await
        .map_err(Into::into)
}
