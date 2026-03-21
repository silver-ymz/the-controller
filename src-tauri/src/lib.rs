use std::sync::Arc;

use tauri::Manager;

pub mod architecture;
pub mod auto_worker;
pub mod broker_client;
pub mod broker_protocol;
pub mod cli_install;
pub mod commands;
pub mod config;
pub mod deploy;
pub mod emitter;
pub mod error;
pub mod generated;
pub mod keybindings;
pub mod labels;
pub mod logging;
pub mod maintainer;
pub mod models;
pub mod note_ai_chat;
pub mod notes;
pub mod pty_manager;
pub mod secure_env;
pub mod service;
pub mod session_args;
pub mod shell_env;
pub mod skills;
pub mod state;
pub mod status_socket;
pub mod storage;
pub mod terminal_theme;
pub mod token_usage;
pub mod voice;
pub mod worktree;

#[cfg(feature = "server")]
pub mod server_helpers;

fn show_startup_error(error: &std::io::Error) {
    tracing::error!("failed to initialize app storage: {error}");
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("The Controller failed to start")
        .set_description(format!(
            "The Controller could not initialize its storage directory.\n\n{}",
            error
        ))
        .show();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Resolve the user's shell environment (e.g. vars from .zshrc) before
    // spawning any threads so PTY sessions inherit them.
    shell_env::inherit_shell_env();

    // Initialize structured logging — must happen before any tracing macros.
    let base_dir = storage::Storage::with_default_path()
        .map(|s| s.base_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let _log_guard = logging::init_backend_logging(&base_dir, true);

    tracing::info!("The Controller starting up");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            tracing::debug!("setting up Tauri plugins and app state");
            let emitter = emitter::TauriEmitter::new(app.handle().clone());
            let app_state = match state::AppState::new(emitter) {
                Ok(state) => state,
                Err(error) => {
                    show_startup_error(&error);
                    std::process::exit(1);
                }
            };
            app.manage(Arc::new(app_state));
            cli_install::install_controller_cli();
            skills::sync_skills();
            {
                let app_state = app.state::<Arc<state::AppState>>();
                let emitter = app_state.emitter.clone();
                let base_dir = app_state.storage.lock().map(|s| s.base_dir()).map_err(|e| {
                    tracing::error!("failed to lock storage for keybindings setup: {e}");
                });
                if let Ok(base_dir) = base_dir {
                    keybindings::ensure_keybindings_file(&base_dir);
                    keybindings::start_watcher(base_dir, emitter);
                }
            }
            status_socket::start_listener(app.handle().clone());
            maintainer::MaintainerScheduler::start(app.handle().clone());
            auto_worker::AutoWorkerScheduler::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            generated::restore_sessions,
            commands::connect_session,
            generated::create_project,
            generated::load_project,
            generated::list_projects,
            generated::delete_project,
            generated::get_agents_md,
            generated::update_agents_md,
            generated::create_session,
            generated::write_to_pty,
            generated::send_raw_to_pty,
            generated::resize_pty,
            generated::close_session,
            generated::set_initial_prompt,
            generated::submit_secure_env_value,
            generated::cancel_secure_env_request,
            generated::start_claude_login,
            generated::stop_claude_login,
            generated::home_dir,
            generated::check_onboarding,
            generated::save_onboarding_config,
            generated::load_terminal_theme,
            commands::check_claude_cli,
            generated::list_directories_at,
            generated::list_root_directories,
            generated::generate_project_names,
            generated::generate_architecture,
            generated::scaffold_project,
            generated::list_github_issues,
            generated::list_assigned_issues,
            generated::generate_issue_body,
            generated::create_github_issue,
            generated::close_github_issue,
            generated::delete_github_issue,
            generated::post_github_comment,
            generated::add_github_label,
            generated::remove_github_label,
            commands::merge_session_branch,
            commands::copy_image_file_to_clipboard,
            commands::capture_app_screenshot,
            generated::get_session_commits,
            generated::configure_maintainer,
            generated::get_maintainer_status,
            commands::get_maintainer_history,
            generated::trigger_maintainer_check,
            generated::clear_maintainer_reports,
            generated::get_maintainer_issues,
            generated::get_maintainer_issue_detail,
            generated::configure_auto_worker,
            generated::get_auto_worker_queue,
            generated::get_worker_reports,
            generated::list_notes,
            generated::read_note,
            generated::write_note,
            generated::create_note,
            generated::rename_note,
            generated::duplicate_note,
            generated::delete_note,
            generated::list_folders,
            generated::create_folder,
            generated::rename_folder,
            generated::delete_folder,
            generated::commit_notes,
            commands::save_note_image,
            generated::resolve_note_asset_path,
            generated::send_note_ai_chat,
            generated::save_session_prompt,
            generated::list_project_prompts,
            commands::stage_session,
            generated::unstage_session,
            generated::get_repo_head,
            generated::get_session_token_usage,
            generated::detect_project_type,
            generated::get_deploy_credentials,
            generated::save_deploy_credentials,
            generated::is_deploy_provisioned,
            generated::deploy_project,
            generated::list_deployed_services,
            generated::start_voice_pipeline,
            generated::stop_voice_pipeline,
            generated::toggle_voice_pause,
            commands::log_frontend_error,
            generated::load_keybindings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                tracing::info!("exit requested, cleaning up");
                status_socket::cleanup();
                // Kill any staged controller instance and clear stale records
                if let Some(state) = app_handle.try_state::<Arc<state::AppState>>() {
                    if let Ok(storage) = state.storage.lock() {
                        if let Ok(inventory) = storage.list_projects() {
                            for project in &inventory.projects {
                                if !project.staged_sessions.is_empty() {
                                    let mut p = project.clone();
                                    for staged in &project.staged_sessions {
                                        commands::kill_process_group(staged.pid);
                                        let _ = std::fs::remove_file(
                                            status_socket::staged_socket_path(&staged.session_id),
                                        );
                                    }
                                    p.staged_sessions.clear();
                                    let _ = storage.save_project(&p);
                                }
                            }
                        }
                    }
                }
                // The broker is a persistent daemon — never shut it down on app exit.
                // Sessions survive and reattach when the app restarts.
            }
        });
}
