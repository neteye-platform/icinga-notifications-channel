//! Derive macros of `icinga-notifications-channel`. Use them through that crate.

mod channel_config;
mod config_enum;
mod locales;
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

/// Describes a struct as the configuration form of a channel.
///
/// Field types map to form elements: `String` to a string (`#[config(text)]`
/// for multi-line), `Secret` to a password, numbers to a number
/// (`#[config(min = 1, max = 9)]`), `bool` to a checkbox, a `ConfigEnum` to a
/// select and a `Vec`/`BTreeSet` of one to a multi-select. A field is
/// required unless it is an `Option` or has a serde default. Names and
/// defaults follow serde.
#[proc_macro_derive(ChannelConfig, attributes(config))]
pub fn channel_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    channel_config::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
