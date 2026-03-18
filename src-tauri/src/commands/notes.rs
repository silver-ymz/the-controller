use tauri::State;

use crate::notes::NoteEntry;
use crate::service;
use crate::state::AppState;

pub(crate) async fn list_notes(
    state: State<'_, AppState>,
    folder: String,
) -> Result<Vec<NoteEntry>, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::list_notes(&storage, &folder).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn read_note(
    state: State<'_, AppState>,
    folder: String,
    filename: String,
) -> Result<String, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::read_note(&storage, &folder, &filename).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn write_note(
    state: State<'_, AppState>,
    folder: String,
    filename: String,
    content: String,
) -> Result<(), String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::write_note(&storage, &folder, &filename, &content).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn create_note(
    state: State<'_, AppState>,
    folder: String,
    title: String,
) -> Result<String, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::create_note(&storage, &folder, &title).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn rename_note(
    state: State<'_, AppState>,
    folder: String,
    old_name: String,
    new_name: String,
) -> Result<String, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::rename_note(&storage, &folder, &old_name, &new_name).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn duplicate_note(
    state: State<'_, AppState>,
    folder: String,
    filename: String,
) -> Result<String, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::duplicate_note(&storage, &folder, &filename).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn delete_note(
    state: State<'_, AppState>,
    folder: String,
    filename: String,
) -> Result<(), String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::delete_note(&storage, &folder, &filename).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn list_folders(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::list_note_folders(&storage).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn create_folder(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::create_note_folder(&storage, &name).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn rename_folder(
    state: State<'_, AppState>,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::rename_note_folder(&storage, &old_name, &new_name).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) async fn delete_folder(
    state: State<'_, AppState>,
    name: String,
    force: bool,
) -> Result<(), String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::delete_note_folder(&storage, &name, force).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Commit any pending note changes (content edits).
/// Called by the frontend when switching notes.
pub(crate) async fn commit_notes(state: State<'_, AppState>) -> Result<bool, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service::commit_pending_notes(&storage).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}
