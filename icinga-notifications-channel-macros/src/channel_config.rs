use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Data, DeriveInput, Error, Expr, Field, Fields, GenericArgument, Ident, PathArguments, Type,
    spanned::Spanned,
};

use crate::serde_attrs::{self, Container, Default};

const NUMBERS: &[&str] = &[
    "i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32", "u64", "usize", "f32", "f64",
];

/// What a field is rendered as.
enum Kind {
    String,
    Text,
    Secret,
    Number,
    Bool,
    /// A `ConfigEnum` type.
    Option(Type),
    /// A collection of a `ConfigEnum` type.
    Options(Type),
}

/// The `#[config(...)]` attributes of a field.
#[derive(Default)]
struct FieldAttrs {
    text: bool,
    secret: bool,
    min: Option<Expr>,
    max: Option<Expr>,
}

fn field_attrs(field: &Field) -> syn::Result<FieldAttrs> {
    let mut attrs = FieldAttrs::default();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("config")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("text") {
                attrs.text = true;
            } else if meta.path.is_ident("secret") {
                attrs.secret = true;
            } else if meta.path.is_ident("min") {
                attrs.min = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("max") {
                attrs.max = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error("unknown option, expected text, secret, min or max"));
            }
            Ok(())
        })?;
    }
    Ok(attrs)
}

/// The single generic argument of `name<T>`, if `ty` is that type.
fn generic_arg<'a>(ty: &'a Type, names: &[&str]) -> Option<&'a Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if !names.iter().any(|n| segment.ident == n) {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

fn classify(ty: &Type, attrs: &FieldAttrs) -> syn::Result<Kind> {
    if let Some(item) = generic_arg(ty, &["Vec", "BTreeSet"]) {
        return Ok(Kind::Options(item.clone()));
    }
    let Type::Path(path) = ty else {
        return Err(Error::new_spanned(
            ty,
            "unsupported field type, expected a string, number, bool, Option, Vec/BTreeSet or ConfigEnum",
        ));
    };
    if path.qself.is_some() {
        return Err(Error::new_spanned(ty, "unsupported field type"));
    }
    let name = path
        .path
        .segments
        .last()
        .expect("paths are not empty")
        .ident
        .to_string();
    Ok(match name.as_str() {
        "String" if attrs.text => Kind::Text,
        "String" if attrs.secret => Kind::Secret,
        "String" => Kind::String,
        "bool" => Kind::Bool,
        n if NUMBERS.contains(&n) => Kind::Number,
        _ => Kind::Option(ty.clone()),
    })
}

fn check_attrs(ty: &Type, kind: &Kind, attrs: &FieldAttrs) -> syn::Result<()> {
    let is_string = matches!(kind, Kind::String | Kind::Text | Kind::Secret);
    if attrs.text && !is_string || attrs.secret && !is_string {
        return Err(Error::new_spanned(
            ty,
            "`text` and `secret` require a String",
        ));
    }
    if attrs.text && attrs.secret {
        return Err(Error::new_spanned(ty, "`text` and `secret` are exclusive"));
    }
    if let (Some(bound), false) = (
        attrs.min.as_ref().or(attrs.max.as_ref()),
        matches!(kind, Kind::Number),
    ) {
        return Err(Error::new_spanned(
            bound,
            "`min` and `max` require a number",
        ));
    }
    Ok(())
}

fn option_name(ident: &Ident, field: &serde_attrs::Field, container: &Container) -> String {
    if let Some(rename) = &field.rename {
        return rename.clone();
    }
    let name = ident.unraw().to_string();
    container
        .rename_all
        .map_or(name.clone(), |rule| rule.apply_to_field(&name))
}

trait Unraw {
    fn unraw(&self) -> Ident;
}

impl Unraw for Ident {
    fn unraw(&self) -> Ident {
        syn::ext::IdentExt::unraw(self)
    }
}

/// Expands `#[derive(ChannelConfig)]`.
pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "ChannelConfig can only be derived for structs",
        ));
    };
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "ChannelConfig does not support generic structs",
        ));
    }
    let Fields::Named(fields) = &data.fields else {
        return Err(Error::new_spanned(
            &input.ident,
            "ChannelConfig requires named fields",
        ));
    };
    let container = serde_attrs::container(&input.attrs)?;
    let ident = &input.ident;
    let krate = quote!(::icinga_notifications_channel);

    let mut options = Vec::new();
    for field in &fields.named {
        let serde = serde_attrs::field(&field.attrs)?;
        if let Some(span) = serde.flatten {
            return Err(Error::new(span, "ChannelConfig does not support flatten"));
        }
        if serde.skip {
            continue;
        }
        let attrs = field_attrs(field)?;
        let field_ident = field.ident.as_ref().expect("named fields");
        let name = option_name(field_ident, &serde, &container);

        let (ty, optional) = match generic_arg(&field.ty, &["Option"]) {
            Some(inner) => (inner, true),
            None => (&field.ty, false),
        };
        let kind = classify(ty, &attrs)?;
        check_attrs(ty, &kind, &attrs)?;

        let config_type = match &kind {
            Kind::String => quote!(String),
            Kind::Text => quote!(Text),
            Kind::Secret => quote!(Secret),
            Kind::Number => quote!(Number),
            Kind::Bool => quote!(Bool),
            Kind::Option(_) => quote!(Option),
            Kind::Options(_) => quote!(Options),
        };
        let mut build = quote! {
            let mut option = #krate::config::ConfigOption::new(
                #name,
                #krate::config::ConfigType::#config_type,
                // Replaced by the locale files.
                #krate::config::Localized::en_us(#name),
            );
        };
        if !optional && serde.default.is_none() && !container.default {
            build.extend(quote!(option = option.required();));
        }
        let default = match (&serde.default, container.default) {
            (Some(Default::Trait), _) => {
                let ty = &field.ty;
                Some(quote!(&<#ty as ::core::default::Default>::default()))
            }
            (Some(Default::Path(path)), _) => Some(quote!(&#path())),
            (None, true) => {
                Some(quote!(&<#ident as ::core::default::Default>::default().#field_ident))
            }
            (None, false) => None,
        };
        if let (Some(default), false) = (default, optional) {
            build.extend(
                quote!(option = option.default(#krate::__private::default_value(#default));),
            );
        }
        if let Some(min) = &attrs.min {
            build.extend(quote!(option = option.min(#min).expect("checked at compile time");));
        }
        if let Some(max) = &attrs.max {
            build.extend(quote!(option = option.max(#max).expect("checked at compile time");));
        }
        if let Kind::Option(ty) | Kind::Options(ty) = &kind {
            build.extend(quote_spanned_options(ty, &krate));
        }
        options.push(quote!({ #build option }));
    }
    Ok(quote! {
        impl #krate::channel::ChannelConfig for #ident {
            fn config_attrs() -> ::std::vec::Vec<#krate::config::ConfigOption> {
                ::std::vec![#(#options),*]
            }
        }
    })
}

fn quote_spanned_options(ty: &Type, krate: &TokenStream) -> TokenStream {
    quote::quote_spanned! {ty.span()=>
        option = option
            .options(
                <#ty as #krate::config::ConfigEnum>::VARIANTS
                    .iter()
                    .map(|variant| (*variant, *variant)),
            )
            .expect("checked at compile time");
    }
}
