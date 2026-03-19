use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Start a Claude login session by spawning a `claude login` PTY command.
/// Should be called from a blocking context.
pub fn start_claude_login(state: &AppState) -> Result<String, AppError> {
    tracing::info!("starting Claude login session");
    let session_id = Uuid::new_v4();
    let mut mgr = state.pty_manager.lock().map_err(AppError::internal)?;
    mgr.spawn_command(session_id, "claude", &["login"], state.emitter.clone())
        .map_err(AppError::Internal)?;
    Ok(session_id.to_string())
}

/// Stop a Claude login session.
pub fn stop_claude_login(state: &AppState, session_id: Uuid) -> Result<(), AppError> {
    let mut mgr = state.pty_manager.lock().map_err(AppError::internal)?;
    mgr.close_session(session_id).map_err(AppError::Internal)
}
