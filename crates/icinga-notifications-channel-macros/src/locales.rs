//! Reads and validates the translation files of a `ChannelConfig`.
//!
//! There is one TOML file per locale, named after it (`en_US.toml`). Each
//! top level table is an option, by its serialized name:
//!
//! ```toml
//! [method]
//! label = "Method"
//! help = "The HTTP method"
//!
//! [method.options]
//! get = "GET"
//! ```

use std::{collections::BTreeMap, fs, path::PathBuf};

use syn::{Error, LitStr};

const EN_US: &str = "en_US";
const KEYS: &[&str] = &["label", "help", "options"];

/// The texts of one option in one locale.
#[derive(Default)]
pub struct Entry {
    pub label: Option<String>,
    pub help: Option<String>,
    /// Select choices by key. Only `en_US` may have them: Icinga
    /// Notifications does not localize them.
    pub options: BTreeMap<String, String>,
}

pub struct File {
    pub locale: String,
    pub path: PathBuf,
    /// The path as written in the attribute, for error messages.
    pub shown: String,
    pub entries: BTreeMap<String, Entry>,
}

fn is_locale(locale: &str) -> bool {
    locale.split_once('_').is_some_and(|(language, region)| {
        (2..=3).contains(&language.len())
            && language.bytes().all(|b| b.is_ascii_lowercase())
            && region.len() == 2
            && region.bytes().all(|b| b.is_ascii_uppercase())
    })
}

/// Loads every `*.toml` file of `dir`, relative to `CARGO_MANIFEST_DIR`.
///
/// `names` are the serialized names of all options. Errors point at `dir`.
pub fn load(dir: &LitStr, names: &[String]) -> syn::Result<Vec<File>> {
    let error = |message: String| Error::new(dir.span(), message);
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| error("CARGO_MANIFEST_DIR is not set".into()))?;
    let dir_path = PathBuf::from(manifest).join(dir.value());
    let read = fs::read_dir(&dir_path).map_err(|e| {
        error(format!(
            "cannot read the locales directory {}: {e}",
            dir_path.display()
        ))
    })?;

    let mut files = Vec::new();
    for entry in read {
        let path = entry.map_err(|e| error(e.to_string()))?.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let locale = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        if !is_locale(&locale) {
            return Err(error(format!(
                "{}/{}: the file name must be a locale in the format language_REGION, e.g. en_US.toml",
                dir.value().trim_end_matches('/'),
                path.file_name().unwrap_or_default().to_string_lossy()
            )));
        }
        let shown = format!("{}/{locale}.toml", dir.value().trim_end_matches('/'));
        let text =
            fs::read_to_string(&path).map_err(|e| error(format!("cannot read {shown}: {e}")))?;
        let entries = parse(&text, names).map_err(|e| error(format!("{shown}: {e}")))?;
        files.push(File {
            locale,
            path,
            shown,
            entries,
        });
    }
    files.sort_by(|a, b| a.locale.cmp(&b.locale));

    let Some(en_us) = files.iter().find(|f| f.locale == EN_US) else {
        return Err(error(format!(
            "{} has no {EN_US}.toml, which is required as the fallback locale",
            dir.value()
        )));
    };
    for name in names {
        if en_us.entries.get(name).is_none_or(|e| e.label.is_none()) {
            return Err(error(format!(
                "{}: option `{name}` has no `label`",
                en_us.shown
            )));
        }
    }
    for file in files.iter().filter(|f| f.locale != EN_US) {
        for (name, entry) in &file.entries {
            if entry.help.is_some() && en_us.entries.get(name).is_none_or(|e| e.help.is_none()) {
                return Err(error(format!(
                    "{}: option `{name}` has a `help` but {EN_US}.toml does not, which is required as the fallback",
                    file.shown
                )));
            }
            if !entry.options.is_empty() {
                return Err(error(format!(
                    "{}: [{name}.options]: choices are not localized by Icinga Notifications, set them in {EN_US}.toml only",
                    file.shown
                )));
            }
        }
    }
    Ok(files)
}

fn parse(text: &str, names: &[String]) -> Result<BTreeMap<String, Entry>, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut entries = BTreeMap::new();
    for (name, value) in table {
        if !names.contains(&name) {
            return Err(format!(
                "unknown option `{name}`, valid options: {}",
                names.join(", ")
            ));
        }
        let toml::Value::Table(fields) = value else {
            return Err(format!("`{name}` must be a table"));
        };
        let mut entry = Entry::default();
        for (key, value) in fields {
            let text = |value: toml::Value| match value {
                toml::Value::String(text) => Ok(text),
                _ => Err(format!("[{name}] `{key}` must be a string")),
            };
            match key.as_str() {
                "label" => entry.label = Some(text(value)?),
                "help" => entry.help = Some(text(value)?),
                "options" => {
                    let toml::Value::Table(options) = value else {
                        return Err(format!("[{name}] `options` must be a table"));
                    };
                    for (choice, label) in options {
                        match label {
                            toml::Value::String(label) => {
                                entry.options.insert(choice, label);
                            }
                            _ => {
                                return Err(format!(
                                    "[{name}.options] `{choice}` must be a string"
                                ));
                            }
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "[{name}] unknown key `{key}`, valid keys: {}",
                        KEYS.join(", ")
                    ));
                }
            }
        }
        entries.insert(name, entry);
    }
    Ok(entries)
}
