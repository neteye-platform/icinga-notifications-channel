use darling::FromMeta;
use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, Ident, ItemFn, Pat, PatType, Type, spanned::Spanned};

#[derive(FromMeta, Default)]
#[darling(derive_syn_parse)]
struct MacroArgs {
    #[darling(default)]
    name: Option<String>,
    #[darling(multiple)]
    state: Vec<String>,
    #[darling(default)]
    params: Option<String>,
}

impl MacroArgs {
    fn is_state(&self, ident: &Ident) -> bool {
        let name = ident.to_string();
        self.state.iter().find(|s| *s == &name).is_some()
    }

    fn is_params(&self, ident: &Ident) -> bool {
        let Some(params) = &self.params else {
            return false;
        };

        let name = ident.to_string();
        params == &name
    }
}

struct PartialError<T> {
    result: T,
    errors: Vec<darling::Error>,
}

#[proc_macro_attribute]
pub fn jsonrpc(args: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as ItemFn);
    let (args, args_error) = match syn::parse::<MacroArgs>(args) {
        Ok(v) => (v, None),
        Err(e) => (MacroArgs::default(), Some(e.to_compile_error())),
    };

    let input_ident = &input.sig.ident;

    let PartialError { result, errors } = match &args.params {
        Some(_) => flattened_impl(&input, &args).into(),
        None => default_impl(&input, &args).into(),
    };

    let errors = errors.into_iter().map(|e| e.write_errors());
    let named = args.name.unwrap_or_else(|| input_ident.to_string());
    quote! {
        #[allow(non_upper_case_globals)]
        const #input_ident: ::jsonrpc_rs::server::JsonRpcNamedMethod = {
            #input

            #result

            #args_error;
            #(#errors);*;

            ::jsonrpc_rs::server::JsonRpcNamedMethod::new(#named, &jsonrpc_dispatch)
        };
    }
    .into()
}

fn default_impl(input: &ItemFn, args: &MacroArgs) -> PartialError<proc_macro2::TokenStream> {
    let mut errors = vec![];
    let mut params_struct_fields = vec![];
    let mut params_state_fields = vec![];
    let mut params_call_fields = vec![];

    for arg in &input.sig.inputs {
        let pat_ty = match arg {
            FnArg::Receiver(_) => {
                errors.push(
                    darling::Error::custom("`self` arguments are not supported").with_span(arg),
                );
                params_call_fields.push(quote! { todo!() });
                continue;
            }
            FnArg::Typed(pat_ty) => pat_ty,
        };

        let ident = match extract_ident(pat_ty) {
            Ok(ident) => ident,
            Err(err) => {
                errors.push(err);
                params_call_fields.push(quote! { todo!() });
                continue;
            }
        };

        if args.is_state(&ident) {
            let PartialError {
                result: (state_fields, call_fields),
                errors: err,
            } = generate_state_code(ident, &pat_ty);

            params_call_fields.push(call_fields);
            params_state_fields.push(state_fields);
            errors.extend(err.into_iter());
        } else {
            params_struct_fields.push(pat_ty);
            params_call_fields.push(quote! { params.#ident });
        }
    }

    let awaitable = if input.sig.asyncness.is_some() {
        quote! { .await }
    } else {
        quote! {}
    };

    let function_name = &input.sig.ident;
    // ToDo: Create diverging branches for `Result`
    let result = quote! {
        #[derive(::jsonrpc_rs::serde::Deserialize)]
        struct Params {
            #(#params_struct_fields),*
        };

        #[allow(dead_code)]
        fn jsonrpc_dispatch<'server, 'request>(
            state: &'server ::jsonrpc_rs::server::ServerState,
            request: &'request ::jsonrpc_rs::server::JsonRpcRequest<'request>,
        ) -> ::std::result::Result<::jsonrpc_rs::server::JsonRpcMethodCall, ::jsonrpc_rs::server::JsonRpcResponseSingle> {
            let params: Params = jsonrpc_rs::server::parse_params(&request)?;

            #(#params_state_fields)*

            Ok(std::boxed::Box::new(async move {
                let result = #function_name (#(#params_call_fields),*) #awaitable;
                ::jsonrpc_rs::server::JsonRpcResponseSingle::new(
                    ::jsonrpc_rs::server::JsonRpcResult::Ok(::std::boxed::Box::new(result))
                ).into()
            }))
        }
    };

    PartialError { result, errors }
}

fn flattened_impl(input: &ItemFn, args: &MacroArgs) -> PartialError<proc_macro2::TokenStream> {
    let mut errors = vec![];
    let mut params_struct_fields = None;
    let mut params_state_fields = vec![];
    let mut params_call_fields = vec![];

    for arg in &input.sig.inputs {
        let pat_ty = match arg {
            FnArg::Receiver(_) => {
                errors.push(
                    darling::Error::custom("`self` arguments are not supported").with_span(arg),
                );
                params_call_fields.push(quote! { todo!() });
                continue;
            }
            FnArg::Typed(pat_ty) => pat_ty,
        };

        let ident = match extract_ident(pat_ty) {
            Ok(ident) => ident,
            Err(err) => {
                errors.push(err);
                params_call_fields.push(quote! { todo!() });
                continue;
            }
        };

        if args.is_state(ident) {
            let PartialError {
                result: (state_fields, call_fields),
                errors: err,
            } = generate_state_code(ident, &pat_ty);

            params_call_fields.push(call_fields);
            params_state_fields.push(state_fields);
            errors.extend(err.into_iter());
        } else if args.is_params(ident) {
            params_struct_fields = Some(pat_ty);
            params_call_fields.push(quote! { single_param });
        }
    }

    let function_name = &input.sig.ident;

    let single_type = match params_struct_fields {
        Some(param) => {
            let ty = &param.ty;
            quote! { #ty }
        }
        None => {
            errors.push(
                darling::Error::custom("indicated single args field not found!")
                    .with_span(&input.sig.inputs),
            );
            quote! { () }
        }
    };

    let awaitable = if input.sig.asyncness.is_some() {
        quote! { .await }
    } else {
        quote! {}
    };

    // ToDo: Create diverging branches for `Result`
    let result = quote! {
        #[allow(dead_code)]
        fn jsonrpc_dispatch<'server, 'request>(
            state: &'server ::jsonrpc_rs::server::ServerState,
            request: &'request ::jsonrpc_rs::server::JsonRpcRequest<'request>,
        ) -> ::std::result::Result<::jsonrpc_rs::server::JsonRpcMethodCall, ::jsonrpc_rs::server::JsonRpcResponseSingle> {
            let single_param: #single_type = jsonrpc_rs::server::parse_params(&request)?;

            #(#params_state_fields)*

            Ok(std::boxed::Box::new(async move {
                let result = #function_name (#(#params_call_fields),*) #awaitable;
                ::jsonrpc_rs::server::JsonRpcResponseSingle::new(
                    ::jsonrpc_rs::server::JsonRpcResult::Ok(::std::boxed::Box::new(result))
                ).into()
            }))
        }
    };

    PartialError { result, errors }
}

fn extract_ident(pt: &PatType) -> Result<&Ident, darling::Error> {
    let Pat::Ident(ident) = pt.pat.as_ref() else {
        return Err(
            darling::Error::custom("cannot extract valid identifier.").with_span(&pt.span())
        );
    };

    Ok(&ident.ident)
}

fn generate_state_code(
    ident: &Ident,
    pat_ty: &PatType,
) -> PartialError<(proc_macro2::TokenStream, proc_macro2::TokenStream)> {
    let mut errors = vec![];

    let field_name = format!("jsonrpc_state_{}", ident);
    let ident = Ident::new(&field_name, ident.span());
    let Type::Reference(ref_ty) = pat_ty.ty.as_ref() else {
        errors.push(darling::Error::custom("can only take state by reference.").with_span(&pat_ty));
        return PartialError {
            result: (quote! {}, quote! { todo!() }),
            errors,
        };
    };

    let ty = ref_ty.elem.as_ref();
    let state_field = quote! {
        let Some(#ident) = state.get_arc_lock::<#ty>() else {
            return Err(::jsonrpc_rs::server::JsonRpcResponseSingle::new(::jsonrpc_rs::server::JsonRpcResult::Err {
                code: ::jsonrpc_rs::server::JsonRpcErrorCode::internal_error(),
                message: "Unmanaged state".to_string(),
                data: ::std::option::Option::None,
            }));
        };
    };

    let call_field = if ref_ty.mutability.is_some() {
        quote! { ::std::ops::Deref::deref(& #ident .write().await) }
    } else {
        quote! { ::std::ops::Deref::deref(& #ident .read().await) }
    };

    PartialError {
        result: (state_field, call_field),
        errors,
    }
}
