use proc_macro::TokenStream;

mod axum_gen;
mod parse;
mod tauri_gen;

/// Auto-derive Tauri commands and Axum handlers from a service function.
///
/// # Flags
///
/// - `tauri_command` — generate a `tauri_<fn_name>` function annotated with
///   `#[tauri::command]`. Parameters are mapped: `&AppState` → `State<'_, AppState>`,
///   `&str` → `String`, `Uuid` → `String` + `parse_uuid()`.
///
/// - `axum_handler` — generate an `axum_<fn_name>` async handler and a
///   `<PascalCase>Request` struct (both behind `#[cfg(feature = "server")]`).
///
/// - `blocking` — wrap the service call with `spawn_blocking` (reserved for Phase B).
///
/// # Supported parameter types
///
/// | Service param | Tauri wrapper | Request struct |
/// |---|---|---|
/// | `&AppState` | `State<'_, AppState>` | skipped |
/// | `&str` | `String` | `String` |
/// | `Uuid` | `String` + parse | `String` |
/// | `bool` | `bool` | `bool` |
/// | other `T` | `T` | `T` |
///
/// # Example
///
/// ```ignore
/// use the_controller_macros::derive_handlers;
///
/// #[derive_handlers(tauri_command, axum_handler)]
/// pub fn create_project(state: &AppState, name: &str, repo_path: &str)
///     -> Result<Project, AppError>
/// {
///     // business logic
/// }
/// ```
///
/// This expands to the original function plus:
/// - `pub fn tauri_create_project(state: State<'_, AppState>, name: String, repo_path: String) -> Result<Project, String>`
/// - `pub struct CreateProjectRequest { pub name: String, pub repo_path: String }` (server only)
/// - `pub async fn axum_create_project(State(state): ..., Json(req): ...) -> Result<Json<Value>, ...>` (server only)
#[proc_macro_attribute]
pub fn derive_handlers(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(attr as parse::DeriveHandlersArgs);
    let item_fn = syn::parse_macro_input!(item as syn::ItemFn);

    match derive_handlers_impl(args, item_fn) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_handlers_impl(
    args: parse::DeriveHandlersArgs,
    item_fn: syn::ItemFn,
) -> syn::Result<proc_macro2::TokenStream> {
    if args.blocking {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "the `blocking` flag is not yet implemented (planned for Phase B)",
        ));
    }

    let parsed = parse::ParsedService::from_item_fn(&item_fn)?;

    // Start with the original function, unchanged
    let mut output = quote::quote! { #item_fn };

    if args.tauri_command {
        let tauri_tokens = tauri_gen::generate_tauri_command(&parsed)?;
        output.extend(tauri_tokens);
    }

    if args.axum_handler {
        let axum_tokens = axum_gen::generate_axum_handler(&parsed)?;
        output.extend(axum_tokens);
    }

    Ok(output)
}
