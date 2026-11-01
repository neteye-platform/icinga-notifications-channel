//! Configuration option descriptions sent in the `GetInfo` response.

use std::{collections::BTreeMap, fmt};

use serde::Serialize;
use serde_json::Value;

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

/// Why a [`ConfigOption`] is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigOptionError {
    /// `min` or `max` on an option that is not a [`ConfigType::Number`].
    BoundsOnNonNumber(String),
    /// `options` on an option that is neither `Option` nor `Options`.
    OptionsOnNonSelect(String),
    /// A child option that has children itself.
    NestedChild(String),
}

impl fmt::Display for ConfigOptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BoundsOnNonNumber(name) => {
                write!(f, "option {name:?}: min and max require the type number")
            }
            Self::OptionsOnNonSelect(name) => write!(
                f,
                "option {name:?}: options require the type option or options"
            ),
            Self::NestedChild(name) => {
                write!(f, "option {name:?}: a child option cannot have children")
            }
        }
    }
}

impl std::error::Error for ConfigOptionError {}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct Common {
    name: String,
    #[serde(rename = "type")]
    kind: ConfigType,
    label: Localized,
    #[serde(skip_serializing_if = "Option::is_none")]
    help: Option<Localized>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<Value>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    required: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    options: BTreeMap<String, String>,
    min: Option<i64>,
    max: Option<i64>,
}

/// One configuration option of a channel, as listed in `config_attrs`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigOption {
    #[serde(flatten)]
    common: Common,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<ChildOption>,
}

/// An option shown only while its parent has one of `parent_values`.
///
/// Children cannot have children.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChildOption {
    #[serde(flatten)]
    common: Common,
    parent_values: Vec<Value>,
}

impl ChildOption {
    /// Makes `option` a child shown for `parent_values`.
    ///
    /// For a select parent, these are the option keys; for a checkbox,
    /// `true` or `false`. Empty means always shown.
    pub fn new(
        option: ConfigOption,
        parent_values: impl IntoIterator<Item = Value>,
    ) -> Result<Self, ConfigOptionError> {
        if !option.children.is_empty() {
            return Err(ConfigOptionError::NestedChild(option.common.name));
        }
        Ok(Self {
            common: option.common,
            parent_values: parent_values.into_iter().collect(),
        })
    }
}

impl ConfigOption {
    /// A new option with the given form element and label.
    pub fn new(name: impl Into<String>, kind: ConfigType, label: Localized) -> Self {
        Self {
            common: Common {
                name: name.into(),
                kind,
                label,
                help: None,
                default: None,
                required: false,
                options: BTreeMap::new(),
                min: None,
                max: None,
            },
            children: Vec::new(),
        }
    }

    /// Sets the tooltip text.
    pub fn help(mut self, help: Localized) -> Self {
        self.common.help = Some(help);
        self
    }

    /// Sets the default value: a bool for checkboxes, a string otherwise.
    pub fn default(mut self, default: impl Into<Value>) -> Self {
        self.common.default = Some(default.into());
        self
    }

    /// Marks the option as required.
    pub fn required(mut self) -> Self {
        self.common.required = true;
        self
    }

    /// Sets the minimum value. Only for [`ConfigType::Number`].
    pub fn min(mut self, min: i64) -> Result<Self, ConfigOptionError> {
        self.require_number()?;
        self.common.min = Some(min);
        Ok(self)
    }

    /// Sets the maximum value. Only for [`ConfigType::Number`].
    pub fn max(mut self, max: i64) -> Result<Self, ConfigOptionError> {
        self.require_number()?;
        self.common.max = Some(max);
        Ok(self)
    }

    /// Sets the selectable `(key, label)` pairs. Only for
    /// [`ConfigType::Option`] and [`ConfigType::Options`].
    pub fn options<K, V>(
        mut self,
        options: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, ConfigOptionError>
    where
        K: Into<String>,
        V: Into<String>,
    {
        if !matches!(self.common.kind, ConfigType::Option | ConfigType::Options) {
            return Err(ConfigOptionError::OptionsOnNonSelect(self.common.name));
        }
        self.common.options = options
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        Ok(self)
    }

    /// Adds a child option.
    pub fn child(mut self, child: ChildOption) -> Self {
        self.children.push(child);
        self
    }

    fn require_number(&self) -> Result<(), ConfigOptionError> {
        if self.common.kind == ConfigType::Number {
            Ok(())
        } else {
            Err(ConfigOptionError::BoundsOnNonNumber(
                self.common.name.clone(),
            ))
        }
    }
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

    fn text(en: &str) -> Localized {
        Localized::en_us(en)
    }

    #[test]
    fn serializes_doc_example() {
        let url = ConfigOption::new(
            "url_template",
            ConfigType::String,
            text("URL Template").with("de_DE", "URL-Template").unwrap(),
        )
        .help(text("URL, optionally as a template."))
        .required();
        let codes = ConfigOption::new("response_status_codes", ConfigType::String, text("Codes"))
            .default("200");
        assert_eq!(
            serde_json::to_value([url, codes]).unwrap(),
            json!([
                {
                    "name": "url_template", "type": "string",
                    "label": {"de_DE": "URL-Template", "en_US": "URL Template"},
                    "help": {"en_US": "URL, optionally as a template."},
                    "required": true, "min": null, "max": null
                },
                {
                    "name": "response_status_codes", "type": "string",
                    "label": {"en_US": "Codes"},
                    "default": "200", "min": null, "max": null
                }
            ])
        );
    }

    #[test]
    fn serializes_bounds_options_and_children() {
        let child = ConfigOption::new("port", ConfigType::Number, text("Port"))
            .min(1)
            .unwrap()
            .max(65535)
            .unwrap();
        let select = ConfigOption::new("mode", ConfigType::Option, text("Mode"))
            .options([("a", "A")])
            .unwrap()
            .child(ChildOption::new(child, [json!("a")]).unwrap());
        let value = serde_json::to_value(select).unwrap();
        assert_eq!(value["options"], json!({"a": "A"}));
        assert_eq!(value["children"][0]["min"], 1);
        assert_eq!(value["children"][0]["max"], 65535);
        assert_eq!(value["children"][0]["parent_values"], json!(["a"]));
    }

    #[test]
    fn rejects_misplaced_attributes() {
        let string = || ConfigOption::new("x", ConfigType::String, text("X"));
        assert!(string().min(1).is_err());
        assert!(string().max(1).is_err());
        assert!(string().options([("a", "A")]).is_err());
        let nested = string().child(ChildOption::new(string(), []).unwrap());
        assert_eq!(
            ChildOption::new(nested, []).unwrap_err(),
            ConfigOptionError::NestedChild("x".into())
        );
    }
}
