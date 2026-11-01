use proc_macro::TokenStream;
use quote::quote;
use syn::{Expr, Ident, ItemFn, Meta, Pat, Token, Type, parse::Parser, punctuated::Punctuated};

#[proc_macro_attribute]
pub fn jsonrpc(args: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as ItemFn);
    let args = MacroArgs::parse(args);

    let name = &input.sig.ident;
    let name_str = name.to_string();
    let params = &input.sig.inputs;
    let method_params = params.iter().filter(|p| match p {
        syn::FnArg::Receiver(_) => false,
        syn::FnArg::Typed(pat_type) => !args.is_state(&pat_type.pat),
    });

    let dist = params.iter().filter_map(|p| match p {
        syn::FnArg::Receiver(_) => None,
        syn::FnArg::Typed(pat_type) => {
            let name = &pat_type.pat;
            if args.is_state(&name) {
                Some(quote! { &*#name.read().await })
            } else {
                Some(quote! { params.#name })
            }
        }
    });

    let method_call = if input.sig.asyncness.is_some() {
        quote! {#name(#(#dist),*).await}
    } else {
        quote! {#name(#(#dist),*)}
    };

    let state = params.iter().filter_map(|p| match p {
        syn::FnArg::Receiver(_) => None,
        syn::FnArg::Typed(pat_type) => {
            let name = &pat_type.pat;
            if args.is_state(&name) {
                let Type::Reference(reference) = &*pat_type.ty else {
                    panic!()
                };
                let ty = &reference.elem;
                Some(quote! {
                    let Some(#name) = state.get_arc_lock::<#ty>() else {
                        return Err(::jsonrpc_rs::server::JsonRpcResponseSingle::new(::jsonrpc_rs::server::JsonRpcResult::Err {
                            code: ::jsonrpc_rs::server::JsonRpcErrorCode::internal_error(),
                            message: "Unmanaged state".to_string(),
                            data: None,
                        }));
                    };
                })
            } else {
                None
            }
        }
    });

    let named = args.name.as_ref().unwrap_or(&name_str);
    quote! {
        #[allow(non_upper_case_globals)]
        const #name: ::jsonrpc_rs::server::JsonRpcNamedMethod = {
            #input

            #[derive(::serde::Deserialize)]
            struct Params {
                #(#method_params),*
            }

            #[allow(dead_code)]
            fn jsonrpc_dispatch<'server, 'request>(
                state: &'server ::jsonrpc_rs::server::ServerState,
                request: &'request ::jsonrpc_rs::server::JsonRpcRequest<'request>,
            ) -> ::std::result::Result<::jsonrpc_rs::server::JsonRpcMethodCall, ::jsonrpc_rs::server::JsonRpcResponseSingle> {
                let params: Params = jsonrpc_rs::server::parse_params(&request)?;
                #(#state)*
                Ok(std::boxed::Box::new(async move {
                    let result = #method_call;
                    ::jsonrpc_rs::server::JsonRpcResponseSingle::new(
                        ::jsonrpc_rs::server::JsonRpcResult::Ok(::std::boxed::Box::new(result))
                    ).into()
                }))
            }

            ::jsonrpc_rs::server::JsonRpcNamedMethod::new(#named, &jsonrpc_dispatch)
        };
    }
    .into()
}

#[derive(Clone, Default)]
struct MacroArgs {
    name: Option<String>,
    state: Vec<String>,
}

impl MacroArgs {
    fn is_state(&self, name: &Pat) -> bool {
        match name {
            Pat::Ident(ident) => self.state.iter().any(|s| &ident.ident == s),
            _ => false,
        }
    }

    fn parse(stream: TokenStream) -> Self {
        let mut args = MacroArgs::default();
        let p = Punctuated::<Meta, Token![,]>::parse_terminated
            .parse(stream)
            .unwrap();

        for field in p {
            match field {
                Meta::Path(_) => panic!("No path field supported!"),
                Meta::List(_) => panic!("No meta_list field supported!"),
                Meta::NameValue(meta_name_value) => {
                    let name = meta_name_value.path.get_ident().unwrap().to_string();
                    if &name == "name" {
                        args.name = Some(as_literal_string(&meta_name_value.value));
                    } else if &name == "state" {
                        args.state.push(as_literal_string(&meta_name_value.value))
                    }
                }
            }
        }

        args
    }
}

fn as_literal_string(expr: &Expr) -> String {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(str),
            ..
        }) => str.value(),
        _ => panic!(),
    }
}
