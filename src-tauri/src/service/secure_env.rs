use crate::error::AppError;
use crate::state::AppState;

/// Submit a secure environment value: take the pending request, write the env
/// file, and finish the submission. Returns "created" or "updated".
pub async fn submit_secure_env_value(
    state: &AppState,
    request_id: &str,
    value: &str,
) -> Result<String, AppError> {
    tracing::debug!(request_id = %request_id, "submitting secure env value");
    let (pending, response_tx) = crate::secure_env::take_secure_env_submission(state, request_id)
        .map_err(AppError::Internal)?;
    let request_id_owned = request_id.to_string();
    let value_owned = value.to_string();
    let result = tokio::task::spawn_blocking(move || {
        crate::secure_env::update_env_file(&pending.env_path, &pending.key, &value_owned)
    })
    .await
    .map_err(|e| AppError::Internal(format!("Task failed: {e}")))?;
    let result =
        crate::secure_env::finish_secure_env_submission(&request_id_owned, response_tx, result)
            .map_err(AppError::Internal)?;
    Ok(if result.created {
        "created".to_string()
    } else {
        "updated".to_string()
    })
}
