use crate::error::AppError;
use crate::state::AppState;
use crate::voice::VoicePipeline;
use the_controller_macros::derive_handlers;

#[derive_handlers(tauri_command, axum_handler)]
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

#[derive_handlers(tauri_command, axum_handler)]
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

#[derive_handlers(tauri_command)]
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
