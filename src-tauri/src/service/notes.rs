use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::AppError;
use crate::note_ai_chat::{NoteAiChatMessage, NoteAiResponse};
use crate::notes::{self, NoteEntry};
use crate::storage::Storage;

/// Best-effort git commit for notes. Logs errors but never fails the caller.
pub fn try_commit_notes(base_dir: &Path, message: &str) {
    tracing::debug!("committing notes");
    if let Err(e) = notes::commit_notes(base_dir, message) {
        tracing::error!(error = %e, "notes git commit failed");
    }
}

pub fn list_notes(storage: &Arc<Mutex<Storage>>, folder: &str) -> Result<Vec<NoteEntry>, AppError> {
    tracing::debug!("listing notes");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::list_notes(&base_dir, folder).map_err(AppError::internal)
}

pub fn read_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    filename: &str,
) -> Result<String, AppError> {
    tracing::debug!("reading note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::read_note(&base_dir, folder, filename).map_err(AppError::internal)
}

pub fn write_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    filename: &str,
    content: &str,
) -> Result<(), AppError> {
    tracing::debug!("writing note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::write_note(&base_dir, folder, filename, content).map_err(AppError::internal)
    // No git commit here — batched via commit_notes command
}

pub fn create_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    title: &str,
) -> Result<String, AppError> {
    tracing::debug!("creating note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    let filename = notes::create_note(&base_dir, folder, title).map_err(AppError::internal)?;
    try_commit_notes(&base_dir, &format!("create {}/{}", folder, filename));
    Ok(filename)
}

pub fn delete_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    filename: &str,
) -> Result<(), AppError> {
    tracing::debug!("deleting note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::delete_note(&base_dir, folder, filename).map_err(AppError::internal)?;
    try_commit_notes(&base_dir, &format!("delete {}/{}", folder, filename));
    Ok(())
}

pub fn rename_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    old_name: &str,
    new_name: &str,
) -> Result<String, AppError> {
    tracing::debug!("renaming note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    let new_filename =
        notes::rename_note(&base_dir, folder, old_name, new_name).map_err(AppError::internal)?;
    try_commit_notes(
        &base_dir,
        &format!("rename {}/{} → {}", folder, old_name, new_filename),
    );
    Ok(new_filename)
}

pub fn duplicate_note(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    filename: &str,
) -> Result<String, AppError> {
    tracing::debug!("duplicating note");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    let copy = notes::duplicate_note(&base_dir, folder, filename).map_err(AppError::internal)?;
    try_commit_notes(
        &base_dir,
        &format!("duplicate {}/{} → {}", folder, filename, copy),
    );
    Ok(copy)
}

pub fn list_note_folders(storage: &Arc<Mutex<Storage>>) -> Result<Vec<String>, AppError> {
    tracing::debug!("listing note folders");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::list_folders(&base_dir).map_err(AppError::internal)
}

pub fn create_note_folder(storage: &Arc<Mutex<Storage>>, name: &str) -> Result<(), AppError> {
    tracing::debug!("creating folder");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::create_folder(&base_dir, name).map_err(AppError::internal)?;
    try_commit_notes(&base_dir, &format!("create folder {}", name));
    Ok(())
}

pub fn rename_note_folder(
    storage: &Arc<Mutex<Storage>>,
    old_name: &str,
    new_name: &str,
) -> Result<(), AppError> {
    tracing::debug!("renaming folder");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::rename_folder(&base_dir, old_name, new_name).map_err(AppError::internal)?;
    try_commit_notes(
        &base_dir,
        &format!("rename folder {} → {}", old_name, new_name),
    );
    Ok(())
}

pub fn delete_note_folder(
    storage: &Arc<Mutex<Storage>>,
    name: &str,
    force: bool,
) -> Result<(), AppError> {
    tracing::debug!(force, "deleting folder");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::delete_folder(&base_dir, name, force).map_err(AppError::internal)?;
    try_commit_notes(&base_dir, &format!("delete folder {}", name));
    Ok(())
}

/// Commit any pending note changes (content edits).
/// Called by the frontend when switching notes.
pub fn commit_pending_notes(storage: &Arc<Mutex<Storage>>) -> Result<bool, AppError> {
    tracing::debug!("committing pending note changes");
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::commit_notes(&base_dir, "update notes").map_err(AppError::internal)
}

pub fn save_note_image(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    image_bytes: &[u8],
    extension: &str,
) -> Result<String, AppError> {
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::save_note_image(&base_dir, folder, image_bytes, extension).map_err(AppError::internal)
}

pub fn resolve_note_asset_path(
    storage: &Arc<Mutex<Storage>>,
    folder: &str,
    relative_path: &str,
) -> Result<String, AppError> {
    let base_dir = storage.lock().map_err(AppError::internal)?.base_dir();
    notes::resolve_note_asset_path(&base_dir, folder, relative_path)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(AppError::internal)
}

pub async fn send_note_ai_chat(
    note_content: String,
    selected_text: String,
    conversation_history: Vec<NoteAiChatMessage>,
    prompt: String,
) -> Result<NoteAiResponse, AppError> {
    crate::note_ai_chat::send_note_ai_message(
        std::env::temp_dir().to_string_lossy().to_string(),
        note_content,
        selected_text,
        conversation_history,
        prompt,
    )
    .await
    .map_err(AppError::Internal)
}
