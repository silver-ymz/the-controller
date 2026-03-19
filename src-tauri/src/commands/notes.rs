use tauri::State;

use crate::notes::NoteEntry;
use crate::service;
use crate::state::AppState;

pub(crate) fn list_notes(
    state: State<'_, AppState>,
    folder: String,
) -> Result<Vec<NoteEntry>, String> {
    service::list_notes(&state, &folder).map_err(Into::into)
}

pub(crate) fn read_note(
    state: State<'_, AppState>,
    folder: String,
    filename: String,
) -> Result<String, String> {
    service::read_note(&state, &folder, &filename).map_err(Into::into)
}

pub(crate) fn write_note(
    state: State<'_, AppState>,
    folder: String,
    filename: String,
    content: String,
) -> Result<(), String> {
    service::write_note(&state, &folder, &filename, &content).map_err(Into::into)
}

pub(crate) fn create_note(
    state: State<'_, AppState>,
    folder: String,
    title: String,
) -> Result<String, String> {
    service::create_note(&state, &folder, &title).map_err(Into::into)
}

pub(crate) fn rename_note(
    state: State<'_, AppState>,
    folder: String,
    old_name: String,
    new_name: String,
) -> Result<String, String> {
    service::rename_note(&state, &folder, &old_name, &new_name).map_err(Into::into)
}
pub(crate) fn duplicate_note(
    state: State<'_, AppState>,
    folder: String,
    filename: String,
) -> Result<String, String> {
    service::duplicate_note(&state, &folder, &filename).map_err(Into::into)
}

pub(crate) fn delete_note(
    state: State<'_, AppState>,
    folder: String,
    filename: String,
) -> Result<(), String> {
    service::delete_note(&state, &folder, &filename).map_err(Into::into)
}

pub(crate) fn list_folders(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    service::list_note_folders(&state).map_err(Into::into)
}

pub(crate) fn create_folder(state: State<'_, AppState>, name: String) -> Result<(), String> {
    service::create_note_folder(&state, &name).map_err(Into::into)
}

pub(crate) fn rename_folder(
    state: State<'_, AppState>,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    service::rename_note_folder(&state, &old_name, &new_name).map_err(Into::into)
}

pub(crate) fn delete_folder(
    state: State<'_, AppState>,
    name: String,
    force: bool,
) -> Result<(), String> {
    service::delete_note_folder(&state, &name, force).map_err(Into::into)
}

/// Commit any pending note changes (content edits).
/// Called by the frontend when switching notes.
pub(crate) fn commit_notes(state: State<'_, AppState>) -> Result<bool, String> {
    service::commit_pending_notes(&state).map_err(Into::into)
}
