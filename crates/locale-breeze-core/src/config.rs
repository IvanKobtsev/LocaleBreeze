use globset::{Glob, GlobMatcher};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    pub dictionaries: String,
    pub default_locale: String,
    #[serde(default = "default_separator")]
    pub key_separator: String,
    pub scoped_functions: Vec<String>,
    pub translation_methods: Vec<String>,
    pub full_key_functions: Vec<String>,
}

fn default_separator() -> String {
    ".".into()
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {0}: {1}")]
    Read(PathBuf, std::io::Error),
    #[error("invalid configuration JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("dictionary pattern must contain exactly one {{locale}} token")]
    LocaleToken,
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("invalid dictionary glob: {0}")]
    Glob(#[from] globset::Error),
    #[error("default locale {0:?} has no matching dictionary")]
    MissingDefaultLocale(String),
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read(path.into(), e))?;
        let config: Self = serde_json::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.dictionaries.matches("{locale}").count() != 1 {
            return Err(ConfigError::LocaleToken);
        }
        if self.default_locale.is_empty() {
            return Err(ConfigError::Empty("defaultLocale"));
        }
        if self.key_separator.is_empty() {
            return Err(ConfigError::Empty("keySeparator"));
        }
        for (name, values) in [
            ("scopedFunctions", &self.scoped_functions),
            ("translationMethods", &self.translation_methods),
            ("fullKeyFunctions", &self.full_key_functions),
        ] {
            if values.is_empty()
                || values
                    .iter()
                    .any(|v| v.is_empty() || v.split('.').any(str::is_empty))
            {
                return Err(ConfigError::Empty(name));
            }
        }
        DictionaryPattern::new(&self.dictionaries)?;
        Ok(())
    }

    pub fn dictionary_pattern(&self) -> Result<DictionaryPattern, ConfigError> {
        DictionaryPattern::new(&self.dictionaries)
    }
}

#[derive(Clone)]
pub struct DictionaryPattern {
    before: String,
    after: String,
    matcher: GlobMatcher,
}

impl DictionaryPattern {
    pub fn new(pattern: &str) -> Result<Self, ConfigError> {
        if pattern.matches("{locale}").count() != 1 {
            return Err(ConfigError::LocaleToken);
        }
        let (before, after) = pattern.split_once("{locale}").unwrap();
        let glob = format!("{}*{}", before.replace('\\', "/"), after.replace('\\', "/"));
        Ok(Self {
            before: before.replace('\\', "/"),
            after: after.replace('\\', "/"),
            matcher: Glob::new(&glob)?.compile_matcher(),
        })
    }

    pub fn locale_for(&self, root: &Path, path: &Path) -> Option<String> {
        let relative = path
            .strip_prefix(root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        if !self.matcher.is_match(&relative) {
            return None;
        }
        let middle = relative
            .strip_prefix(&self.before)?
            .strip_suffix(&self.after)?;
        (!middle.is_empty() && !middle.contains('/')).then(|| middle.to_owned())
    }
}
