use proc_macro::TokenStream;
use quote::quote;
use syn::ItemFn;

#[proc_macro_attribute]
pub fn jsonrpc(_: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as ItemFn);
    let name = &input.sig.ident;
    let name_str = name.to_string();
    let params = &input.sig.inputs;

    let dist = params.iter().filter_map(|p| match p {
        syn::FnArg::Receiver(_) => None,
        syn::FnArg::Typed(pat_type) => Some(&pat_type.pat),
    });

    quote! {
        #[allow(non_upper_case_globals)]
        const #name: ::jsonrpc_rs::server::JsonRpcNamedMethod = {
            #input

            #[derive(::serde::Deserialize)]
            struct Params {
                #params
            }

            #[allow(dead_code)]
            fn jsonrpc_dispatch<'server, 'request>(
                state: &'server ::jsonrpc_rs::server::ServerState,
                request: &'request ::jsonrpc_rs::server::JsonRpcRequest<'request>,
            ) -> ::std::result::Result<::jsonrpc_rs::server::JsonRpcMethodCall, ::jsonrpc_rs::server::JsonRpcResponseSingle> {
                let params: Params = jsonrpc_rs::server::parse_params(&request)?;
                Ok(std::boxed::Box::new(async move {
                    let result = #name(#(params.#dist),*).await;
                    ::jsonrpc_rs::server::JsonRpcResponseSingle::new(
                        ::jsonrpc_rs::server::JsonRpcResult::Ok(::std::boxed::Box::new(result))
                    ).into()
                }))
            }

            ::jsonrpc_rs::server::JsonRpcNamedMethod::new(#name_str, &jsonrpc_dispatch)
        };
    }
    .into()
}
