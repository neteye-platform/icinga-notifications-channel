//! Configuration option descriptions sent in the `GetInfo` response.

use std::{collections::BTreeMap, fmt};

use serde::Serialize;

/// A locale in the `language_REGION` format, e.g. `en_US`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Locale(String);

impl Locale {
    /// The fallback locale every [`Localized`] text must have.
    pub const EN_US: &'static str = "en_US";

    /// Validates `locale` as `language_REGION`: 2-3 lowercase letters, an
    /// underscore and 2 uppercase letters.
    pub fn new(locale: impl Into<String>) -> Result<Self, LocaleError> {
        let locale = locale.into();
        let valid = locale.split_once('_').is_some_and(|(language, region)| {
            (2..=3).contains(&language.len())
                && language.bytes().all(|b| b.is_ascii_lowercase())
                && region.len() == 2
                && region.bytes().all(|b| b.is_ascii_uppercase())
        });
        if valid {
            Ok(Self(locale))
        } else {
            Err(LocaleError::InvalidLocale(locale))
        }
    }

    /// The locale as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a [`Localized`] text could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocaleError {
    /// The locale is not in the `language_REGION` format.
    InvalidLocale(String),
    /// There is no `en_US` entry.
    MissingEnUs,
}

impl fmt::Display for LocaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLocale(locale) => write!(
                f,
                "invalid locale {locale:?}, expected the format language_REGION, e.g. en_US"
            ),
            Self::MissingEnUs => write!(f, "the en_US locale is required as a fallback"),
        }
    }
}

impl std::error::Error for LocaleError {}

/// Texts by locale. Always contains an `en_US` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Localized(BTreeMap<Locale, String>);

impl Localized {
    /// A text with only the `en_US` entry.
    pub fn en_us(text: impl Into<String>) -> Self {
        let locale = Locale::new(Locale::EN_US).expect("en_US is a valid locale");
        Self(BTreeMap::from([(locale, text.into())]))
    }

    /// Builds the texts from `(locale, text)` pairs.
    ///
    /// Fails if a locale is invalid or `en_US` is missing.
    pub fn try_from_pairs<L, T>(
        pairs: impl IntoIterator<Item = (L, T)>,
    ) -> Result<Self, LocaleError>
    where
        L: Into<String>,
        T: Into<String>,
    {
        let mut map = BTreeMap::new();
        for (locale, text) in pairs {
            map.insert(Locale::new(locale)?, text.into());
        }
        if map.keys().any(|l| l.as_str() == Locale::EN_US) {
            Ok(Self(map))
        } else {
            Err(LocaleError::MissingEnUs)
        }
    }

    /// Adds or replaces the text of `locale`.
    pub fn with(
        mut self,
        locale: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<Self, LocaleError> {
        self.0.insert(Locale::new(locale)?, text.into());
        Ok(self)
    }
}

/// The kind of form element Icinga Notifications Web renders for an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigType {
    /// Single-line input.
    String,
    /// Number input.
    Number,
    /// Checkbox.
    Bool,
    /// Multi-line input.
    Text,
    /// Select.
    Option,
    /// Multi-select.
    Options,
    /// Password input.
    Secret,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_missing_en_us() {
        let error = Localized::try_from_pairs([("de_DE", "x")]).unwrap_err();
        assert_eq!(error, LocaleError::MissingEnUs);
    }

    #[test]
    fn rejects_invalid_locale_naming_it() {
        let error = Localized::try_from_pairs([("en-US", "x")]).unwrap_err();
        assert!(error.to_string().contains("\"en-US\""), "{error}");
        for bad in ["", "en", "en_", "EN_us", "e_US", "en_USA", "_US"] {
            assert!(Locale::new(bad).is_err(), "{bad}");
        }
        assert!(Locale::new("de_DE").is_ok());
    }

    #[test]
    fn serializes_as_map() {
        let l = Localized::en_us("Save").with("de_DE", "Speichern").unwrap();
        assert_eq!(
            serde_json::to_value(l).unwrap(),
            json!({"en_US": "Save", "de_DE": "Speichern"})
        );
    }

    #[test]
    fn config_type_uses_go_names() {
        use ConfigType::*;
        let all = [String, Number, Bool, Text, Option, Options, Secret];
        assert_eq!(
            serde_json::to_value(all).unwrap(),
            json!([
                "string", "number", "bool", "text", "option", "options", "secret"
            ])
        );
    }
}
