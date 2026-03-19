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
///   `&str` → `String`, `Uuid` → `String` + `parse_uuid()`, `&[u8]` → `String` + `.as_bytes()`.
///
/// - `axum_handler` — generate an `axum_<fn_name>` async handler and a
///   `<PascalCase>Request` struct (both behind `#[cfg(feature = "server")]`).
///
/// - `blocking` — wrap the service call with `spawn_blocking` for functions that
///   perform blocking I/O. Uses `tauri::async_runtime::spawn_blocking` for Tauri
///   and `tokio::task::spawn_blocking` for Axum.
///
/// # Async support
///
/// If the service function is `async fn`, the generated Tauri wrapper will be
/// `async fn` and `.await` the service call. Axum handlers always add `.await`
/// for async service functions.
///
/// # Supported parameter types
///
/// | Service param | Tauri wrapper | Request struct |
/// |---|---|---|
/// | `&AppState` | `State<'_, AppState>` | skipped |
/// | `&str` | `String` | `String` |
/// | `&[u8]` | `String` + `.as_bytes()` | `String` |
/// | `Uuid` | `String` + parse | `String` |
/// | `bool` | `bool` | `bool` |
/// | other `T` | `T` | `T` |
///
/// Functions without an `&AppState` parameter will generate handlers that omit
/// the state extractor entirely.
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
    let parsed = parse::ParsedService::from_item_fn(&item_fn)?;

    // Start with the original function, unchanged
    let mut output = quote::quote! { #item_fn };

    if args.tauri_command {
        let tauri_tokens = tauri_gen::generate_tauri_command(&parsed, args.blocking)?;
        output.extend(tauri_tokens);
    }

    if args.axum_handler {
        let axum_tokens = axum_gen::generate_axum_handler(&parsed, args.blocking)?;
        output.extend(axum_tokens);
    }

    Ok(output)
}
