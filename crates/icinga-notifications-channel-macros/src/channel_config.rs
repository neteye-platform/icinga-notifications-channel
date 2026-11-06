use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::{
    Data, DeriveInput, Error, Expr, ExprArray, ExprLit, Field, Fields, GenericArgument, Ident, Lit,
    LitStr, PathArguments, Type, spanned::Spanned,
};

use crate::{
    locales,
    serde_attrs::{self, Container, Default},
};

/// The `#[config(...)]` attributes of a field.
#[derive(Default)]
struct FieldAttrs {
    text: bool,
    min: Option<Expr>,
    max: Option<Expr>,
    /// `parent = "field"`: the field whose value decides if this one shows.
    parent: Option<LitStr>,
    /// `when = [...]`: the parent values this one shows for.
    when: Option<ExprArray>,
}

fn field_attrs(field: &Field) -> syn::Result<FieldAttrs> {
    let mut attrs = FieldAttrs::default();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("config")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("text") {
                attrs.text = true;
            } else if meta.path.is_ident("min") {
                attrs.min = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("max") {
                attrs.max = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("parent") {
                attrs.parent = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("when") {
                attrs.when = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error("unknown option, expected text, min, max, parent or when"));
            }
            Ok(())
        })?;
    }
    match (&attrs.parent, &attrs.when) {
        (Some(_), None) => {
            return Err(Error::new_spanned(
                field,
                "`parent` requires `when = [...]`",
            ));
        }
        (None, Some(when)) => {
            return Err(Error::new_spanned(when, "`when` requires `parent`"));
        }
        _ => {}
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

/// `#[config(locales = "dir")]` on the struct.
fn locales_dir(input: &DeriveInput) -> syn::Result<LitStr> {
    let mut dir = None;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("config")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("locales") {
                dir = Some(meta.value()?.parse::<LitStr>()?);
                Ok(())
            } else {
                Err(meta.error("unknown option, expected locales"))
            }
        })?;
    }
    dir.ok_or_else(|| {
        Error::new_spanned(
            &input.ident,
            "ChannelConfig requires `#[config(locales = \"dir\")]`, the directory of the translation files",
        )
    })
}

/// The texts of `files` for `name`, as a `Localized` expression.
fn localized(
    files: &[locales::File],
    name: &str,
    text: impl Fn(&locales::Entry) -> Option<&String>,
    krate: &TokenStream,
) -> Option<TokenStream> {
    let pairs: Vec<_> = files
        .iter()
        .filter_map(|file| {
            let text = text(file.entries.get(name)?)?;
            let locale = &file.locale;
            Some(quote!((#locale, #text)))
        })
        .collect();
    (!pairs.is_empty()).then(|| {
        quote! {
            #krate::config::Localized::try_from_pairs([#(#pairs),*])
                .expect("checked at compile time")
        }
    })
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

    let mut names = Vec::new();
    for field in &fields.named {
        let serde = serde_attrs::field(&field.attrs)?;
        if !serde.skip {
            let field_ident = field.ident.as_ref().expect("named fields");
            names.push(option_name(field_ident, &serde, &container));
        }
    }
    let dir = locales_dir(input)?;
    let files = locales::load(&dir, &names)?;
    let tracked = files.iter().map(|file| {
        let path = file.path.to_string_lossy();
        quote!(
            const _: &str = include_str!(#path);
        )
    });

    let mut items: Vec<Item> = Vec::new();
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
        let kind = if attrs.text {
            quote!(#krate::config::ConfigType::Text)
        } else {
            quote!(<#ty as #krate::config::FieldType>::KIND)
        };
        let checks = type_checks(ty, &attrs, &krate)?;
        let label = localized(&files, &name, |e| e.label.as_ref(), &krate)
            .expect("en_US labels are checked by locales::load");
        let mut build = quote! {
            let mut option = #krate::config::ConfigOption::new(#name, #kind, #label);
        };
        if let Some(help) = localized(&files, &name, |e| e.help.as_ref(), &krate) {
            build.extend(quote!(option = option.help(#help);));
        }
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
        let overrides = files
            .iter()
            .find(|f| f.locale == "en_US")
            .and_then(|f| f.entries.get(&name))
            .map(|entry| {
                let pairs = entry.options.iter().map(|(k, v)| quote!((#k, #v)));
                quote! {
                    .map(|(key, label)| {
                        let overrides: &[(&str, &str)] = &[#(#pairs),*];
                        let label = overrides
                            .iter()
                            .find(|(k, _)| *k == key)
                            .map_or(label, |(_, label)| *label);
                        (key, label)
                    })
                }
            });
        build.extend(quote! {
            if matches!(
                <#ty as #krate::config::FieldType>::KIND,
                #krate::config::ConfigType::Option | #krate::config::ConfigType::Options
            ) {
                option = option
                    .options(
                        <#ty as #krate::config::FieldType>::CHOICES
                            .iter()
                            .map(|choice| (*choice, *choice))
                            #overrides,
                    )
                    .expect("checked above");
            }
        });
        let parent = match (&attrs.parent, &attrs.when) {
            (Some(parent), Some(when)) => Some(Parent {
                field: parent.clone(),
                when: when.clone(),
            }),
            _ => None,
        };
        items.push(Item {
            field: field_ident.unraw().to_string(),
            ty: ty.clone(),
            checks,
            build,
            parent,
            children: Vec::new(),
        });
    }
    let options = attach_children(items, &krate)?;
    Ok(quote! {
        #(#tracked)*

        impl #krate::channel::ChannelConfig for #ident {
            fn config_attrs() -> ::std::vec::Vec<#krate::config::ConfigOption> {
                ::std::vec![#(#options),*]
            }
        }
    })
}

/// Compile-time assertions that the attributes fit the type's [`FieldType`].
fn type_checks(ty: &Type, attrs: &FieldAttrs, krate: &TokenStream) -> syn::Result<TokenStream> {
    let mut checks = TokenStream::new();
    let mut require = |kind: TokenStream, message: &str| {
        checks.extend(quote_spanned! {ty.span()=>
            const _: () = assert!(
                matches!(
                    <#ty as #krate::config::FieldType>::KIND,
                    #krate::config::ConfigType::#kind
                ),
                #message
            );
        });
    };
    if attrs.text {
        require(quote!(String), "`text` requires a String");
    }
    if attrs.min.is_some() || attrs.max.is_some() {
        require(quote!(Number), "`min` and `max` require a number");
    }
    Ok(checks)
}

struct Parent {
    field: LitStr,
    when: ExprArray,
}

struct Item {
    field: String,
    /// The field type without `Option`.
    ty: Type,
    checks: TokenStream,
    build: TokenStream,
    parent: Option<Parent>,
    /// Builders of the child options, already wrapped in `ChildOption`.
    children: Vec<TokenStream>,
}

/// Moves every child under its parent and returns the top level builders.
fn attach_children(mut items: Vec<Item>, krate: &TokenStream) -> syn::Result<Vec<TokenStream>> {
    for child in 0..items.len() {
        let Some(parent) = items[child].parent.take() else {
            continue;
        };
        let Some(index) = items.iter().position(|i| parent.field.value() == i.field) else {
            return Err(Error::new_spanned(
                &parent.field,
                format!("`{}` is not a field of this struct", parent.field.value()),
            ));
        };
        if items[index].parent.is_some() || items[index].field == items[child].field {
            return Err(Error::new_spanned(
                &parent.field,
                "a child cannot be a parent, nor its own parent",
            ));
        }
        let parent_ty = items[index].ty.clone();
        let values = parent
            .when
            .elems
            .iter()
            .map(|value| parent_value(value, &parent_ty, krate))
            .collect::<syn::Result<Vec<_>>>()?;
        let checks = values.iter().map(|(_, check)| check);
        let values = values.iter().map(|(value, _)| value);
        let Item {
            checks: child_checks,
            build,
            ..
        } = &items[child];
        let tokens = quote! {
            {
                #(#checks)*
                #child_checks
                #build
                #krate::config::ChildOption::new(option, [#(#values),*])
                    .expect("children cannot have children, checked at compile time")
            }
        };
        items[index].children.push(tokens);
        // Placeholder so indexes stay valid; removed below.
        items[child].parent = Some(parent);
    }
    Ok(items
        .into_iter()
        .filter(|item| item.parent.is_none())
        .map(|item| {
            let Item {
                checks,
                build,
                children,
                ..
            } = item;
            quote!({
                #checks
                #build
                #(option = option.child(#children);)*
                option
            })
        })
        .collect())
}

/// The JSON value of a `when` entry and a compile-time check that the
/// parent can have it.
fn parent_value(
    value: &Expr,
    parent_ty: &Type,
    krate: &TokenStream,
) -> syn::Result<(TokenStream, TokenStream)> {
    let span = value.span();
    match value {
        Expr::Lit(ExprLit {
            lit: Lit::Str(choice),
            ..
        }) => Ok((
            quote!(#krate::__private::Value::from(#choice)),
            quote_spanned! {span=>
                const _: () = assert!(
                    #krate::__private::contains(
                        <#parent_ty as #krate::config::FieldType>::CHOICES,
                        #choice,
                    ),
                    "the parent has no such option value",
                );
            },
        )),
        Expr::Lit(ExprLit {
            lit: Lit::Bool(flag),
            ..
        }) => Ok((
            quote!(#krate::__private::Value::from(#flag)),
            quote_spanned! {span=>
                const _: () = assert!(
                    matches!(
                        <#parent_ty as #krate::config::FieldType>::KIND,
                        #krate::config::ConfigType::Bool
                    ),
                    "true and false require a bool parent",
                );
            },
        )),
        _ => Err(Error::new_spanned(
            value,
            "expected a string literal (select option) or true/false (checkbox)",
        )),
    }
}
