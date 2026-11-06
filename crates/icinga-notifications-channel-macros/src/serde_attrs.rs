//! The parts of `#[serde(...)]` that decide the names and defaults of
//! configuration options.

use syn::{Attribute, Expr, LitStr, Token, parenthesized, token};

/// `#[serde(rename_all = "...")]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    fn parse(rule: &str) -> Option<Self> {
        Some(match rule {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            _ => return None,
        })
    }

    /// Renames an enum variant, written in `PascalCase`.
    pub fn apply_to_variant(self, variant: &str) -> String {
        match self {
            Self::Pascal => variant.to_owned(),
            Self::Lower => variant.to_lowercase(),
            Self::Upper => variant.to_uppercase(),
            Self::Camel => {
                let mut chars = variant.chars();
                chars
                    .next()
                    .map(|first| first.to_lowercase().chain(chars).collect())
                    .unwrap_or_default()
            }
            Self::Snake => pascal_to_snake(variant),
            Self::ScreamingSnake => pascal_to_snake(variant).to_uppercase(),
            Self::Kebab => pascal_to_snake(variant).replace('_', "-"),
            Self::ScreamingKebab => pascal_to_snake(variant).to_uppercase().replace('_', "-"),
        }
    }

    /// Renames a struct field, written in `snake_case`.
    pub fn apply_to_field(self, field: &str) -> String {
        match self {
            Self::Lower | Self::Snake => field.to_owned(),
            Self::Upper | Self::ScreamingSnake => field.to_uppercase(),
            Self::Pascal => snake_to_pascal(field),
            Self::Camel => {
                let pascal = snake_to_pascal(field);
                let mut chars = pascal.chars();
                chars
                    .next()
                    .map(|first| first.to_lowercase().chain(chars).collect())
                    .unwrap_or_default()
            }
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => field.to_uppercase().replace('_', "-"),
        }
    }
}

fn pascal_to_snake(name: &str) -> String {
    let mut snake = String::new();
    for (i, c) in name.char_indices() {
        if c.is_uppercase() && i > 0 {
            snake.push('_');
        }
        snake.extend(c.to_lowercase());
    }
    snake
}

fn snake_to_pascal(name: &str) -> String {
    name.split('_')
        .flat_map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<Vec<_>>())
        })
        .flatten()
        .collect()
}

/// How `#[serde(default)]` fills a missing field.
#[derive(Debug, Clone, PartialEq)]
pub enum Default {
    /// `#[serde(default)]`: `Default::default()`.
    Trait,
    /// `#[serde(default = "path")]`: calls the function.
    Path(syn::Path),
}

/// Container level serde attributes.
#[derive(Debug, Default)]
pub struct Container {
    pub rename_all: Option<RenameRule>,
    /// `#[serde(default)]` on the struct: missing fields come from
    /// `Default::default()` of the whole struct.
    pub default: bool,
}

/// Field level serde attributes.
#[derive(Debug, Default)]
pub struct Field {
    pub rename: Option<String>,
    pub default: Option<Default>,
    pub skip: bool,
    pub flatten: Option<proc_macro2::Span>,
}

/// Consumes the value of an attribute this module does not care about.
fn skip_value(meta: &syn::meta::ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(Token![=]) {
        meta.value()?.parse::<Expr>()?;
    } else if meta.input.peek(token::Paren) {
        let content;
        parenthesized!(content in meta.input);
        content.parse::<proc_macro2::TokenStream>()?;
    }
    Ok(())
}

/// Reads `rename = "x"` or `rename(serialize = "x", deserialize = "y")`,
/// preferring the deserialize name: that is what `SetConfig` uses.
fn rename_value(meta: &syn::meta::ParseNestedMeta) -> syn::Result<String> {
    if meta.input.peek(Token![=]) {
        return Ok(meta.value()?.parse::<LitStr>()?.value());
    }
    let mut name = None;
    meta.parse_nested_meta(|inner| {
        if inner.path.is_ident("deserialize") {
            name = Some(inner.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else {
            skip_value(&inner)
        }
    })?;
    name.ok_or_else(|| meta.error("expected rename(deserialize = \"...\")"))
}

fn for_each_serde(
    attrs: &[Attribute],
    mut f: impl FnMut(&syn::meta::ParseNestedMeta) -> syn::Result<()>,
) -> syn::Result<()> {
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| f(&meta))?;
    }
    Ok(())
}

pub fn container(attrs: &[Attribute]) -> syn::Result<Container> {
    let mut container = Container::default();
    for_each_serde(attrs, |meta| {
        if meta.path.is_ident("rename_all") {
            let rule = rename_value(meta)?;
            container.rename_all = Some(
                RenameRule::parse(&rule)
                    .ok_or_else(|| meta.error(format!("unknown rename rule {rule:?}")))?,
            );
            Ok(())
        } else if meta.path.is_ident("default") {
            if meta.input.peek(Token![=]) {
                return Err(meta.error(
                    "`default = \"path\"` on the struct is not supported, use `#[serde(default)]`",
                ));
            }
            container.default = true;
            Ok(())
        } else {
            skip_value(meta)
        }
    })?;
    Ok(container)
}

pub fn field(attrs: &[Attribute]) -> syn::Result<Field> {
    let mut field = Field::default();
    for_each_serde(attrs, |meta| {
        if meta.path.is_ident("rename") {
            field.rename = Some(rename_value(meta)?);
        } else if meta.path.is_ident("default") {
            field.default = Some(if meta.input.peek(Token![=]) {
                Default::Path(meta.value()?.parse::<LitStr>()?.parse()?)
            } else {
                Default::Trait
            });
        } else if meta.path.is_ident("skip") || meta.path.is_ident("skip_deserializing") {
            field.skip = true;
        } else if meta.path.is_ident("flatten") {
            field.flatten = Some(meta.path.segments[0].ident.span());
        } else {
            skip_value(meta)?;
        }
        Ok(())
    })?;
    Ok(field)
}

/// The name of a variant in the serialized form.
pub fn variant_name(
    ident: &syn::Ident,
    attrs: &[Attribute],
    rule: Option<RenameRule>,
) -> syn::Result<String> {
    let mut rename = None;
    for_each_serde(attrs, |meta| {
        if meta.path.is_ident("rename") {
            rename = Some(rename_value(meta)?);
            Ok(())
        } else {
            skip_value(meta)
        }
    })?;
    Ok(rename.unwrap_or_else(|| {
        let name = ident.to_string();
        rule.map_or(name.clone(), |rule| rule.apply_to_variant(&name))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_variants() {
        use RenameRule::*;
        assert_eq!(Lower.apply_to_variant("GetRequest"), "getrequest");
        assert_eq!(Upper.apply_to_variant("GetRequest"), "GETREQUEST");
        assert_eq!(Pascal.apply_to_variant("GetRequest"), "GetRequest");
        assert_eq!(Camel.apply_to_variant("GetRequest"), "getRequest");
        assert_eq!(Snake.apply_to_variant("GetRequest"), "get_request");
        assert_eq!(ScreamingSnake.apply_to_variant("GetRequest"), "GET_REQUEST");
        assert_eq!(Kebab.apply_to_variant("GetRequest"), "get-request");
        assert_eq!(ScreamingKebab.apply_to_variant("GetRequest"), "GET-REQUEST");
    }

    #[test]
    fn renames_fields() {
        use RenameRule::*;
        assert_eq!(Lower.apply_to_field("url_template"), "url_template");
        assert_eq!(Upper.apply_to_field("url_template"), "URL_TEMPLATE");
        assert_eq!(Pascal.apply_to_field("url_template"), "UrlTemplate");
        assert_eq!(Camel.apply_to_field("url_template"), "urlTemplate");
        assert_eq!(Snake.apply_to_field("url_template"), "url_template");
        assert_eq!(
            ScreamingSnake.apply_to_field("url_template"),
            "URL_TEMPLATE"
        );
        assert_eq!(Kebab.apply_to_field("url_template"), "url-template");
        assert_eq!(
            ScreamingKebab.apply_to_field("url_template"),
            "URL-TEMPLATE"
        );
    }
}
