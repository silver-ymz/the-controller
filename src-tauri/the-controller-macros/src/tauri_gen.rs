use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::parse::{ParamKind, ParsedService};

/// Generate a `#[tauri::command]` wrapper function for the given service function.
pub fn generate_tauri_command(parsed: &ParsedService, blocking: bool) -> syn::Result<TokenStream> {
    let service_fn_name = &parsed.fn_name;
    let tauri_fn_name = format_ident!("tauri_{}", service_fn_name);
    let ok_type = &parsed.ok_type;

    let mut wrapper_params = Vec::new();
    let mut call_args = Vec::new();
    let mut uuid_parse_stmts = Vec::new();

    for param in &parsed.params {
        match &param.kind {
            ParamKind::AppState => {
                let name = &param.name;
                wrapper_params.push(quote! {
                    #name: ::tauri::State<'_, ::std::sync::Arc<crate::state::AppState>>
                });
                call_args.push(quote! { &#name });
            }
            ParamKind::StrRef => {
                let name = &param.name;
                wrapper_params.push(quote! { #name: String });
                call_args.push(quote! { &#name });
            }
            ParamKind::UuidParam => {
                let name = &param.name;
                let parsed_name = format_ident!("{}_uuid", name);
                wrapper_params.push(quote! { #name: String });
                uuid_parse_stmts.push(quote! {
                    let #parsed_name = crate::commands::parse_uuid(&#name)?;
                });
                call_args.push(quote! { #parsed_name });
            }
            ParamKind::Bool => {
                let name = &param.name;
                wrapper_params.push(quote! { #name: bool });
                call_args.push(quote! { #name });
            }
            ParamKind::ByteSlice => {
                let name = &param.name;
                wrapper_params.push(quote! { #name: String });
                call_args.push(quote! { #name.as_bytes() });
            }
            ParamKind::OptionStrRef => {
                let name = &param.name;
                wrapper_params.push(quote! { #name: Option<String> });
                call_args.push(quote! { #name.as_deref() });
            }
            ParamKind::Passthrough(ty) => {
                let name = &param.name;
                wrapper_params.push(quote! { #name: #ty });
                call_args.push(quote! { #name });
            }
        }
    }

    let service_call = quote! { #service_fn_name(#(#call_args),*) };

    let output = if blocking {
        // Blocking: clone state, move owned params into spawn_blocking closure
        let has_state = parsed
            .params
            .iter()
            .any(|p| matches!(p.kind, ParamKind::AppState));

        let state_clone = if has_state {
            // Find the state param name
            let state_name = parsed
                .params
                .iter()
                .find(|p| matches!(p.kind, ParamKind::AppState))
                .map(|p| &p.name)
                .unwrap();
            quote! { let #state_name = (*#state_name).clone(); }
        } else {
            quote! {}
        };

        quote! {
            #[tauri::command]
            pub async fn #tauri_fn_name(#(#wrapper_params),*) -> Result<#ok_type, String> {
                #state_clone
                ::tauri::async_runtime::spawn_blocking(move || {
                    #(#uuid_parse_stmts)*
                    #service_call.map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| format!("Task failed: {e}"))?
            }
        }
    } else if parsed.is_async {
        // Async non-blocking: async fn with .await on service call
        quote! {
            #[tauri::command]
            pub async fn #tauri_fn_name(#(#wrapper_params),*) -> Result<#ok_type, String> {
                #(#uuid_parse_stmts)*
                #service_call.await.map_err(|e| e.to_string())
            }
        }
    } else {
        // Sync non-blocking: plain fn
        quote! {
            #[tauri::command]
            pub fn #tauri_fn_name(#(#wrapper_params),*) -> Result<#ok_type, String> {
                #(#uuid_parse_stmts)*
                #service_call.map_err(|e| e.to_string())
            }
        }
    };

    Ok(output)
}
