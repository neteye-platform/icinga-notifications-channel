//! Derive macros of `icinga-notifications-channel`. Use them through that crate.

mod config_enum;
// Field helpers are used by `ChannelConfig` in a later change.
#[allow(dead_code)]
mod serde_attrs;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

/// Lists the serialized variant names of a unit-only enum as the choices of
/// a select option.
///
/// Names follow `#[serde(rename)]` and `#[serde(rename_all)]`.
#[proc_macro_derive(ConfigEnum)]
pub fn config_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    config_enum::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
