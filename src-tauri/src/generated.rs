//! Forwarding wrappers that map generated `tauri_*` function names to the
//! original Tauri IPC command names expected by the frontend.
//!
//! The `#[derive_handlers]` macro on service functions produces wrappers
//! named `tauri_<fn_name>`. Tauri's `generate_handler!` macro uses the
//! function's ident as the IPC command name, so registering `tauri_list_projects`
//! would expose command `"tauri_list_projects"` — breaking the frontend which
//! calls `"list_projects"`.
//!
//! This module defines thin `#[tauri::command]` functions with the original
//! names that simply forward to the generated `tauri_*` functions.

use std::sync::Arc;

use crate::state::AppState;

/// Declare a forwarding `#[tauri::command]` function.
macro_rules! fwd {
    // Async variant
    (async fn $name:ident => $($gen:ident)::+ ( $($arg:ident : $ty:ty),* $(,)? ) -> Result<$ok:ty, String>) => {
        #[tauri::command]
        pub async fn $name( $($arg: $ty),* ) -> Result<$ok, String> {
            $($gen)::+( $($arg),* ).await
        }
    };
    // Sync variant
    (fn $name:ident => $($gen:ident)::+ ( $($arg:ident : $ty:ty),* $(,)? ) -> Result<$ok:ty, String>) => {
        #[tauri::command]
        pub fn $name( $($arg: $ty),* ) -> Result<$ok, String> {
            $($gen)::+( $($arg),* )
        }
    };
}

type S<'a> = ::tauri::State<'a, Arc<AppState>>;

// --- projects ---
fwd!(fn list_projects => crate::service::tauri_list_projects(state: S<'_>) -> Result<crate::storage::ProjectInventory, String>);
fwd!(fn create_project => crate::service::tauri_create_project(state: S<'_>, name: String, repo_path: String) -> Result<crate::models::Project, String>);
fwd!(fn check_onboarding => crate::service::tauri_check_onboarding(state: S<'_>) -> Result<Option<crate::config::Config>, String>);
fwd!(fn load_project => crate::service::tauri_load_project(state: S<'_>, name: String, repo_path: String) -> Result<crate::models::Project, String>);
fwd!(async fn delete_project => crate::service::tauri_delete_project(state: S<'_>, project_id: String, delete_repo: bool) -> Result<(), String>);
fwd!(fn get_agents_md => crate::service::tauri_get_agents_md(state: S<'_>, project_id: String) -> Result<String, String>);
fwd!(fn update_agents_md => crate::service::tauri_update_agents_md(state: S<'_>, project_id: String, content: String) -> Result<(), String>);

// --- scaffold ---
fwd!(async fn scaffold_project => crate::service::tauri_scaffold_project(state: S<'_>, name: String) -> Result<crate::models::Project, String>);

// --- sessions ---
fwd!(async fn restore_sessions => crate::service::tauri_restore_sessions(state: S<'_>) -> Result<(), String>);
fwd!(async fn create_session => crate::service::tauri_create_session(state: S<'_>, project_id: String, session_id: String, kind: String, github_issue: Option<crate::models::GithubIssue>, background: bool, initial_prompt: Option<String>) -> Result<String, String>);
fwd!(fn close_session => crate::service::tauri_close_session(state: S<'_>, project_id: String, session_id: String, delete_worktree: bool) -> Result<(), String>);
fwd!(fn write_to_pty => crate::service::tauri_write_to_pty(state: S<'_>, session_id: String, data: String) -> Result<(), String>);
fwd!(fn send_raw_to_pty => crate::service::tauri_send_raw_to_pty(state: S<'_>, session_id: String, data: String) -> Result<(), String>);
fwd!(fn resize_pty => crate::service::tauri_resize_pty(state: S<'_>, session_id: String, rows: u16, cols: u16) -> Result<(), String>);
fwd!(fn set_initial_prompt => crate::service::tauri_set_initial_prompt(state: S<'_>, project_id: String, session_id: String, prompt: String) -> Result<(), String>);
fwd!(fn unstage_session => crate::service::tauri_unstage_session(state: S<'_>, project_id: String, session_id: String) -> Result<(), String>);
fwd!(async fn get_session_commits => crate::service::tauri_get_session_commits(state: S<'_>, project_id: String, session_id: String) -> Result<Vec<crate::models::CommitInfo>, String>);
fwd!(async fn get_session_token_usage => crate::service::tauri_get_session_token_usage(state: S<'_>, project_id: String, session_id: String) -> Result<Vec<crate::token_usage::TokenDataPoint>, String>);
fwd!(fn save_session_prompt => crate::service::tauri_save_session_prompt(state: S<'_>, project_id: String, session_id: String) -> Result<(), String>);
fwd!(fn list_project_prompts => crate::service::tauri_list_project_prompts(state: S<'_>, project_id: String) -> Result<Vec<crate::models::SavedPrompt>, String>);
fwd!(async fn get_repo_head => crate::service::tauri_get_repo_head(repo_path: String) -> Result<(String, String), String>);

// --- github ---
fwd!(async fn list_github_issues => crate::service::tauri_list_github_issues(state: S<'_>, repo_path: String) -> Result<Vec<crate::models::GithubIssue>, String>);
fwd!(async fn list_assigned_issues => crate::service::tauri_list_assigned_issues(repo_path: String) -> Result<Vec<crate::models::AssignedIssue>, String>);
fwd!(async fn generate_issue_body => crate::service::tauri_generate_issue_body(title: String) -> Result<String, String>);
fwd!(async fn create_github_issue => crate::service::tauri_create_github_issue(state: S<'_>, repo_path: String, title: String, body: String) -> Result<crate::models::GithubIssue, String>);
fwd!(async fn close_github_issue => crate::service::tauri_close_github_issue(state: S<'_>, repo_path: String, issue_number: u64, comment: String) -> Result<(), String>);
fwd!(async fn delete_github_issue => crate::service::tauri_delete_github_issue(state: S<'_>, repo_path: String, issue_number: u64) -> Result<(), String>);
fwd!(async fn post_github_comment => crate::service::tauri_post_github_comment(repo_path: String, issue_number: u64, body: String) -> Result<(), String>);
fwd!(async fn add_github_label => crate::service::tauri_add_github_label(state: S<'_>, repo_path: String, issue_number: u64, label: String, description: Option<String>, color: Option<String>) -> Result<(), String>);
fwd!(async fn remove_github_label => crate::service::tauri_remove_github_label(state: S<'_>, repo_path: String, issue_number: u64, label: String) -> Result<(), String>);
fwd!(async fn get_worker_reports => crate::service::tauri_get_worker_reports(repo_path: String) -> Result<Vec<crate::service::WorkerReport>, String>);
fwd!(async fn get_maintainer_issues => crate::service::tauri_get_maintainer_issues_for_project(state: S<'_>, project_id: String) -> Result<Vec<crate::models::MaintainerIssue>, String>);
fwd!(async fn get_maintainer_issue_detail => crate::service::tauri_get_maintainer_issue_detail_for_project(state: S<'_>, project_id: String, issue_number: u32) -> Result<crate::models::MaintainerIssueDetail, String>);

// --- maintainer ---
fwd!(fn configure_maintainer => crate::service::tauri_configure_maintainer(state: S<'_>, project_id: String, enabled: bool, interval_minutes: u64, github_repo: Option<String>) -> Result<(), String>);
fwd!(fn configure_auto_worker => crate::service::tauri_configure_auto_worker(state: S<'_>, project_id: String, enabled: bool) -> Result<(), String>);
fwd!(fn get_maintainer_status => crate::service::tauri_get_maintainer_status(state: S<'_>, project_id: String) -> Result<Option<crate::models::MaintainerRunLog>, String>);
fwd!(async fn trigger_maintainer_check => crate::service::tauri_trigger_maintainer_check(state: S<'_>, project_id: String) -> Result<crate::models::MaintainerRunLog, String>);
fwd!(fn clear_maintainer_reports => crate::service::tauri_clear_maintainer_reports(state: S<'_>, project_id: String) -> Result<(), String>);
fwd!(async fn get_auto_worker_queue => crate::service::tauri_get_auto_worker_queue(state: S<'_>, project_id: String) -> Result<Vec<crate::models::AutoWorkerQueueIssue>, String>);

// --- notes ---
fwd!(async fn list_notes => crate::service::tauri_list_notes(state: S<'_>, folder: String) -> Result<Vec<crate::notes::NoteEntry>, String>);
fwd!(async fn read_note => crate::service::tauri_read_note(state: S<'_>, folder: String, filename: String) -> Result<String, String>);
fwd!(async fn write_note => crate::service::tauri_write_note(state: S<'_>, folder: String, filename: String, content: String) -> Result<(), String>);
fwd!(async fn create_note => crate::service::tauri_create_note(state: S<'_>, folder: String, title: String) -> Result<String, String>);
fwd!(async fn rename_note => crate::service::tauri_rename_note(state: S<'_>, folder: String, old_name: String, new_name: String) -> Result<String, String>);
fwd!(async fn duplicate_note => crate::service::tauri_duplicate_note(state: S<'_>, folder: String, filename: String) -> Result<String, String>);
fwd!(async fn delete_note => crate::service::tauri_delete_note(state: S<'_>, folder: String, filename: String) -> Result<(), String>);
fwd!(async fn list_folders => crate::service::tauri_list_note_folders(state: S<'_>) -> Result<Vec<String>, String>);
fwd!(async fn create_folder => crate::service::tauri_create_note_folder(state: S<'_>, name: String) -> Result<(), String>);
fwd!(async fn rename_folder => crate::service::tauri_rename_note_folder(state: S<'_>, old_name: String, new_name: String) -> Result<(), String>);
fwd!(async fn delete_folder => crate::service::tauri_delete_note_folder(state: S<'_>, name: String, force: bool) -> Result<(), String>);
fwd!(async fn commit_notes => crate::service::tauri_commit_pending_notes(state: S<'_>) -> Result<bool, String>);
fwd!(async fn save_note_image => crate::service::tauri_save_note_image(state: S<'_>, folder: String, image_bytes: String, extension: String) -> Result<String, String>);
fwd!(async fn resolve_note_asset_path => crate::service::tauri_resolve_note_asset_path(state: S<'_>, folder: String, relative_path: String) -> Result<String, String>);
fwd!(async fn send_note_ai_chat => crate::service::tauri_send_note_ai_chat(note_content: String, selected_text: String, conversation_history: Vec<crate::note_ai_chat::NoteAiChatMessage>, prompt: String) -> Result<crate::note_ai_chat::NoteAiResponse, String>);

// --- config ---
fwd!(fn home_dir => crate::service::tauri_home_dir() -> Result<String, String>);
fwd!(fn list_directories_at => crate::service::tauri_list_directories_at(path: String) -> Result<Vec<crate::config::DirEntry>, String>);
fwd!(fn list_root_directories => crate::service::tauri_list_root_directories(state: S<'_>) -> Result<Vec<crate::config::DirEntry>, String>);
fwd!(async fn generate_project_names => crate::service::tauri_generate_project_names(description: String) -> Result<Vec<String>, String>);
fwd!(async fn generate_architecture => crate::service::tauri_generate_architecture(state: S<'_>, repo_path: String) -> Result<crate::architecture::ArchitectureResult, String>);
fwd!(async fn load_terminal_theme => crate::service::tauri_load_terminal_theme_blocking(state: S<'_>) -> Result<crate::terminal_theme::TerminalTheme, String>);
fwd!(fn load_keybindings => crate::service::tauri_load_keybindings(state: S<'_>) -> Result<crate::keybindings::KeybindingsResult, String>);

// --- auth ---
fwd!(async fn start_claude_login => crate::service::tauri_start_claude_login(state: S<'_>) -> Result<String, String>);
fwd!(fn stop_claude_login => crate::service::tauri_stop_claude_login(state: S<'_>, session_id: String) -> Result<(), String>);

// --- deploy ---
fwd!(async fn detect_project_type => crate::service::tauri_detect_project_type_blocking(repo_path: String) -> Result<crate::deploy::commands::ProjectSignals, String>);
fwd!(async fn get_deploy_credentials => crate::service::tauri_get_deploy_credentials_blocking() -> Result<crate::deploy::credentials::DeployCredentials, String>);
fwd!(async fn save_deploy_credentials => crate::service::tauri_save_deploy_credentials_blocking(credentials: crate::deploy::credentials::DeployCredentials) -> Result<(), String>);
fwd!(async fn is_deploy_provisioned => crate::service::tauri_is_deploy_provisioned_blocking() -> Result<bool, String>);
fwd!(async fn deploy_project => crate::service::tauri_deploy_project(request: crate::deploy::commands::DeployRequest) -> Result<crate::deploy::commands::DeployResult, String>);
fwd!(async fn list_deployed_services => crate::service::tauri_list_deployed_services() -> Result<Vec<serde_json::Value>, String>);

// --- secure_env ---
fwd!(async fn submit_secure_env_value => crate::service::tauri_submit_secure_env_value(state: S<'_>, request_id: String, value: String) -> Result<String, String>);
fwd!(fn cancel_secure_env_request => crate::service::tauri_cancel_secure_env_request(state: S<'_>, request_id: String) -> Result<(), String>);

// --- voice ---
fwd!(async fn start_voice_pipeline => crate::service::tauri_start_voice_pipeline(state: S<'_>) -> Result<(), String>);
fwd!(async fn stop_voice_pipeline => crate::service::tauri_stop_voice_pipeline(state: S<'_>) -> Result<(), String>);
fwd!(async fn toggle_voice_pause => crate::service::tauri_toggle_voice_pause(state: S<'_>) -> Result<bool, String>);
