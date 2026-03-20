use std::path::Path;

use crate::config;
use crate::error::AppError;
use crate::keybindings;
use crate::state::AppState;
use crate::storage::ProjectInventory;
use crate::terminal_theme;
use the_controller_macros::derive_handlers;

#[derive_handlers(tauri_command, axum_handler)]
pub fn home_dir() -> Result<String, AppError> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| AppError::Internal("Could not determine home directory".to_string()))
}

/// Check the Claude CLI installation and authentication status.
/// This spawns a subprocess and should be called from a blocking context.
#[derive_handlers(axum_handler, blocking)]
pub fn check_claude_cli() -> Result<String, AppError> {
    Ok(config::check_claude_cli_status())
}

/// Save onboarding config with a projects root and optional default provider.
/// If `default_provider` is `None`, defaults to `ClaudeCode`.
#[derive_handlers(tauri_command, axum_handler)]
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

#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn load_terminal_theme_blocking(
    state: &AppState,
) -> Result<terminal_theme::TerminalTheme, AppError> {
    let base_dir = state.storage.lock().map_err(AppError::internal)?.base_dir();
    terminal_theme::load_terminal_theme(&base_dir).map_err(AppError::internal)
}

#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn generate_architecture(
    state: &AppState,
    repo_path: &str,
) -> Result<crate::architecture::ArchitectureResult, AppError> {
    crate::architecture::generate_architecture_blocking_with_emitter(
        std::path::Path::new(repo_path),
        &state.emitter,
    )
    .map_err(AppError::Internal)
}

#[derive_handlers(tauri_command, axum_handler)]
pub fn load_keybindings(state: &AppState) -> Result<keybindings::KeybindingsResult, AppError> {
    let base_dir = state.storage.lock().map_err(AppError::internal)?.base_dir();
    Ok(keybindings::load_keybindings(&base_dir))
}

/// Log a frontend error to the dedicated log file and tracing.
#[derive_handlers(axum_handler)]
pub fn log_frontend_error(state: &AppState, message: &str) -> Result<(), AppError> {
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
    Ok(())
}

#[derive_handlers(tauri_command)]
pub fn list_directories_at(path: &str) -> Result<Vec<config::DirEntry>, AppError> {
    let p = Path::new(path);
    if !p.is_dir() {
        return Err(AppError::BadRequest(format!("Not a directory: {}", path)));
    }
    config::list_directories(p).map_err(AppError::internal)
}

#[derive_handlers(tauri_command, axum_handler)]
pub fn list_root_directories(state: &AppState) -> Result<Vec<config::DirEntry>, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let base_dir = storage.base_dir();
    let cfg = config::load_config(&base_dir).ok_or_else(|| {
        AppError::BadRequest("No config found. Complete onboarding first.".to_string())
    })?;
    config::list_directories(Path::new(&cfg.projects_root)).map_err(AppError::internal)
}

#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn generate_project_names(description: &str) -> Result<Vec<String>, AppError> {
    config::generate_names_via_cli(description).map_err(AppError::Internal)
}

#[derive_handlers(axum_handler)]
pub fn list_archived_projects(state: &AppState) -> Result<ProjectInventory, AppError> {
    let storage = state.storage.lock().map_err(AppError::internal)?;
    let inventory = storage.list_projects().map_err(AppError::internal)?;
    Ok(inventory.filter_projects(|project| {
        project.archived || project.sessions.iter().any(|session| session.archived)
    }))
}

#[derive_handlers(axum_handler)]
pub fn list_directories_at_safe(path: &str) -> Result<Vec<config::DirEntry>, AppError> {
    let p = Path::new(path);
    let requested = std::fs::canonicalize(p)
        .map_err(|e| AppError::BadRequest(format!("cannot resolve path: {}", e)))?;
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::Internal("cannot determine home directory".to_string()))?;
    if !requested.starts_with(&home) {
        return Err(AppError::Forbidden(
            "path must be under the home directory".to_string(),
        ));
    }
    list_directories_at(path)
}
