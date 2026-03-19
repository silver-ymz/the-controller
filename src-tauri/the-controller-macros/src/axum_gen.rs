use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::parse::{ParamKind, ParsedParam, ParsedService};

/// Generate an Axum HTTP handler + request struct for the given service function.
/// Everything is gated behind `#[cfg(feature = "server")]`.
pub fn generate_axum_handler(parsed: &ParsedService) -> syn::Result<TokenStream> {
    let service_fn_name = &parsed.fn_name;
    let axum_fn_name = format_ident!("axum_{}", service_fn_name);
    let request_struct_name = to_pascal_case_request(&service_fn_name.to_string());
    let request_struct_ident = format_ident!("{}", request_struct_name);

    // Collect non-AppState params for the request struct
    let request_fields: Vec<&ParsedParam> = parsed
        .params
        .iter()
        .filter(|p| !matches!(p.kind, ParamKind::AppState))
        .collect();

    // Build request struct fields
    let struct_fields: Vec<TokenStream> = request_fields
        .iter()
        .map(|p| {
            let name = &p.name;
            let ty = request_field_type(&p.kind);
            quote! { pub #name: #ty }
        })
        .collect();

    // Build the request struct (might be empty for zero-arg endpoints)
    let request_struct = if struct_fields.is_empty() {
        quote! {}
    } else {
        quote! {
            #[cfg(feature = "server")]
            #[derive(::serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            pub struct #request_struct_ident {
                #(#struct_fields),*
            }
        }
    };

    // Build handler body: extract from req, parse uuids, call service fn
    let mut uuid_parse_stmts = Vec::new();
    let mut call_args = Vec::new();

    for param in &parsed.params {
        match &param.kind {
            ParamKind::AppState => {
                call_args.push(quote! { &state.app });
            }
            ParamKind::StrRef => {
                let name = &param.name;
                call_args.push(quote! { &req.#name });
            }
            ParamKind::UuidParam => {
                let name = &param.name;
                let parsed_name = format_ident!("{}_uuid", name);
                uuid_parse_stmts.push(quote! {
                    let #parsed_name = crate::server_helpers::parse_uuid(&req.#name)?;
                });
                call_args.push(quote! { #parsed_name });
            }
            ParamKind::Bool => {
                let name = &param.name;
                call_args.push(quote! { req.#name });
            }
            ParamKind::Passthrough(_) => {
                let name = &param.name;
                call_args.push(quote! { req.#name });
            }
        }
    }

    // Handler function — with or without Json extractor
    let handler = if struct_fields.is_empty() {
        quote! {
            #[cfg(feature = "server")]
            pub async fn #axum_fn_name(
                ::axum::extract::State(state): ::axum::extract::State<
                    ::std::sync::Arc<crate::server_helpers::ServerState>,
                >,
            ) -> Result<::axum::Json<::serde_json::Value>, (::axum::http::StatusCode, String)> {
                #(#uuid_parse_stmts)*
                let result = #service_fn_name(#(#call_args),*)
                    .map_err(<(::axum::http::StatusCode, String)>::from)?;
                crate::server_helpers::ok_json(result)
            }
        }
    } else {
        quote! {
            #[cfg(feature = "server")]
            pub async fn #axum_fn_name(
                ::axum::extract::State(state): ::axum::extract::State<
                    ::std::sync::Arc<crate::server_helpers::ServerState>,
                >,
                ::axum::Json(req): ::axum::Json<#request_struct_ident>,
            ) -> Result<::axum::Json<::serde_json::Value>, (::axum::http::StatusCode, String)> {
                #(#uuid_parse_stmts)*
                let result = #service_fn_name(#(#call_args),*)
                    .map_err(<(::axum::http::StatusCode, String)>::from)?;
                crate::server_helpers::ok_json(result)
            }
        }
    };

    Ok(quote! {
        #request_struct
        #handler
    })
}

/// Map a `ParamKind` to the type used in the request struct.
fn request_field_type(kind: &ParamKind) -> TokenStream {
    match kind {
        ParamKind::AppState => unreachable!("AppState is filtered out before this point"),
        ParamKind::StrRef | ParamKind::UuidParam => quote! { String },
        ParamKind::Bool => quote! { bool },
        ParamKind::Passthrough(ty) => quote! { #ty },
    }
}

/// Convert `snake_case` to `PascalCaseRequest`.
/// e.g. `create_project` → `CreateProjectRequest`
fn to_pascal_case_request(s: &str) -> String {
    let mut result = String::new();
    for word in s.split('_') {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            result.push(first.to_uppercase().next().unwrap_or(first));
            result.extend(chars);
        }
    }
    result.push_str("Request");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case_request_simple() {
        assert_eq!(
            to_pascal_case_request("list_projects"),
            "ListProjectsRequest"
        );
    }

    #[test]
    fn pascal_case_request_single_word() {
        assert_eq!(to_pascal_case_request("list"), "ListRequest");
    }

    #[test]
    fn pascal_case_request_three_words() {
        assert_eq!(
            to_pascal_case_request("get_agents_md"),
            "GetAgentsMdRequest"
        );
    }
}
