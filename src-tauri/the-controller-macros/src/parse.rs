use syn::parse::{Parse, ParseStream};
use syn::{FnArg, Ident, ItemFn, Pat, PatType, ReturnType, Token, Type, TypeReference};

/// Flags parsed from the `#[derive_handlers(...)]` attribute.
pub struct DeriveHandlersArgs {
    pub tauri_command: bool,
    pub axum_handler: bool,
    pub blocking: bool,
}

impl Parse for DeriveHandlersArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = Self {
            tauri_command: false,
            axum_handler: false,
            blocking: false,
        };

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "tauri_command" => args.tauri_command = true,
                "axum_handler" => args.axum_handler = true,
                "blocking" => args.blocking = true,
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unknown derive_handlers flag: `{other}`"),
                    ))
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

/// Classification of a service function parameter for code generation.
pub enum ParamKind {
    /// `&AppState` — injected from Tauri/Axum state extractors, skipped in request struct.
    AppState,
    /// `&str` — becomes `String` in the wrapper, passed as `&arg` to service fn.
    StrRef,
    /// `&[u8]` — becomes `String` in the wrapper, passed as `arg.as_bytes()` to service fn.
    ByteSlice,
    /// `Uuid` — becomes `String` in the wrapper, parsed with `parse_uuid()`.
    UuidParam,
    /// `bool` — passed through as-is.
    Bool,
    /// `Option<&str>` — becomes `Option<String>`, passed as `arg.as_deref()`.
    OptionStrRef,
    /// Any other type — passed through as-is (e.g. `u16`, `Option<String>`).
    Passthrough(Box<Type>),
}

/// A parsed parameter from the service function signature.
pub struct ParsedParam {
    pub name: Ident,
    pub kind: ParamKind,
    /// The original type from the signature (used in error messages).
    #[allow(dead_code)] // reserved for future diagnostics
    pub original_ty: Type,
}

/// Fully parsed service function, ready for code generation.
pub struct ParsedService {
    pub fn_name: Ident,
    #[allow(dead_code)] // reserved for visibility-aware generation
    pub vis: syn::Visibility,
    pub params: Vec<ParsedParam>,
    /// The `T` in `Result<T, AppError>`.
    pub ok_type: Type,
    /// Whether the service function is `async fn`.
    pub is_async: bool,
}

impl ParsedService {
    pub fn from_item_fn(item: &ItemFn) -> syn::Result<Self> {
        let fn_name = item.sig.ident.clone();
        let vis = item.vis.clone();
        let is_async = item.sig.asyncness.is_some();

        // Parse return type: must be Result<T, AppError>
        let ok_type = extract_result_ok_type(&item.sig.output)?;

        // Parse parameters
        let mut params = Vec::new();
        for arg in &item.sig.inputs {
            let param = parse_fn_arg(arg)?;
            params.push(param);
        }

        Ok(Self {
            fn_name,
            vis,
            params,
            ok_type,
            is_async,
        })
    }
}

fn extract_result_ok_type(ret: &ReturnType) -> syn::Result<Type> {
    let ty = match ret {
        ReturnType::Type(_, ty) => ty.as_ref(),
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                ret,
                "derive_handlers requires a return type of Result<T, AppError>",
            ))
        }
    };

    // Match Result<T, ...>
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Result" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(ok_ty)) = args.args.first() {
                        return Ok(ok_ty.clone());
                    }
                }
            }
        }
    }

    Err(syn::Error::new_spanned(
        ty,
        "derive_handlers requires a return type of Result<T, AppError>",
    ))
}

fn parse_fn_arg(arg: &FnArg) -> syn::Result<ParsedParam> {
    let typed = match arg {
        FnArg::Typed(t) => t,
        FnArg::Receiver(_) => {
            return Err(syn::Error::new_spanned(
                arg,
                "derive_handlers does not support `self` parameters",
            ))
        }
    };

    let name = extract_param_name(typed)?;
    let kind = classify_type(&typed.ty);

    Ok(ParsedParam {
        name,
        kind,
        original_ty: (*typed.ty).clone(),
    })
}

fn extract_param_name(typed: &PatType) -> syn::Result<Ident> {
    match typed.pat.as_ref() {
        Pat::Ident(pat_ident) => Ok(pat_ident.ident.clone()),
        other => Err(syn::Error::new_spanned(
            other,
            "derive_handlers requires simple parameter names (no destructuring)",
        )),
    }
}

fn classify_type(ty: &Type) -> ParamKind {
    // Check for &AppState
    if is_ref_to(ty, "AppState") {
        return ParamKind::AppState;
    }
    // Check for &str
    if is_ref_to(ty, "str") {
        return ParamKind::StrRef;
    }
    // Check for &[u8]
    if is_ref_to_byte_slice(ty) {
        return ParamKind::ByteSlice;
    }
    // Check for Uuid
    if is_path_ending_with(ty, "Uuid") {
        return ParamKind::UuidParam;
    }
    // Check for bool
    if is_path_ending_with(ty, "bool") {
        return ParamKind::Bool;
    }
    // Check for Option<&str>
    if is_option_str_ref(ty) {
        return ParamKind::OptionStrRef;
    }
    // Everything else: pass through
    ParamKind::Passthrough(Box::new(ty.clone()))
}

fn is_ref_to(ty: &Type, target: &str) -> bool {
    if let Type::Reference(TypeReference { elem, .. }) = ty {
        return is_path_ending_with(elem, target);
    }
    false
}

fn is_ref_to_byte_slice(ty: &Type) -> bool {
    if let Type::Reference(TypeReference { elem, .. }) = ty {
        if let Type::Slice(type_slice) = elem.as_ref() {
            return is_path_ending_with(&type_slice.elem, "u8");
        }
    }
    false
}

fn is_path_ending_with(ty: &Type, target: &str) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            return seg.ident == target;
        }
    }
    false
}

/// Check for `Option<&str>` — matches `Option<&str>` specifically.
fn is_option_str_ref(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            if seg.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return is_ref_to(inner, "str");
                    }
                }
            }
        }
    }
    false
}
