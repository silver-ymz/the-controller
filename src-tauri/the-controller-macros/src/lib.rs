#[allow(dead_code)] // Will be wired up when derive_handlers is fully implemented
mod axum_gen;
#[allow(dead_code)] // Used by tauri_gen and axum_gen modules (not yet wired up)
mod parse;
#[allow(dead_code)] // Will be wired up when derive_handlers is fully implemented
mod tauri_gen;

use proc_macro::TokenStream;

/// Auto-derive Tauri commands and Axum handlers from a service function.
///
/// # Flags
/// - `tauri_command` — generate a `#[tauri::command]` wrapper
/// - `axum_handler` — generate an Axum handler + request struct (behind `#[cfg(feature = "server")]`)
/// - `blocking` — wrap the service call with `spawn_blocking` (reserved, not yet implemented)
///
/// # Example
/// ```ignore
/// #[derive_handlers(tauri_command, axum_handler)]
/// pub fn list_projects(state: &AppState) -> Result<ProjectInventory, AppError> { ... }
/// ```
#[proc_macro_attribute]
pub fn derive_handlers(attr: TokenStream, item: TokenStream) -> TokenStream {
    let _ = attr;
    // Stub: pass through the original item unchanged
    item
}
