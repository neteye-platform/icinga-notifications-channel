use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Error, Fields};

use crate::serde_attrs;

/// Expands `#[derive(ConfigEnum)]`.
pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Enum(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "ConfigEnum can only be derived for enums",
        ));
    };
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "ConfigEnum does not support generic enums",
        ));
    }
    let rule = serde_attrs::container(&input.attrs)?.rename_all;
    let mut names = Vec::new();
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(Error::new_spanned(
                variant,
                "ConfigEnum only supports unit variants",
            ));
        }
        names.push(serde_attrs::variant_name(
            &variant.ident,
            &variant.attrs,
            rule,
        )?);
    }
    let ident = &input.ident;
    Ok(quote! {
        impl ::icinga_notifications_channel::config::ConfigEnum for #ident {
            const VARIANTS: &'static [&'static str] = &[#(#names),*];
        }

        impl ::icinga_notifications_channel::config::FieldType for #ident {
            const KIND: ::icinga_notifications_channel::config::ConfigType =
                ::icinga_notifications_channel::config::ConfigType::Option;
            const CHOICES: &'static [&'static str] =
                <Self as ::icinga_notifications_channel::config::ConfigEnum>::VARIANTS;
        }
    })
}
