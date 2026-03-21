use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use the_controller_macros::derive_handlers;

#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn start_claude_login(state: &AppState) -> Result<String, AppError> {
    tracing::info!("starting Claude login session");
    let session_id = Uuid::new_v4();
    let mut mgr = state.pty_manager.lock().map_err(AppError::internal)?;
    mgr.spawn_command(session_id, "claude", &["login"], state.emitter.clone())
        .map_err(AppError::Internal)?;
    Ok(session_id.to_string())
}

#[derive_handlers(tauri_command, axum_handler)]
pub fn stop_claude_login(state: &AppState, session_id: Uuid) -> Result<(), AppError> {
    let mut mgr = state.pty_manager.lock().map_err(AppError::internal)?;
    mgr.close_session(session_id).map_err(AppError::Internal)
}
