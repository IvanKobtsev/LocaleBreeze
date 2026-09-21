use globset::{Glob, GlobMatcher};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashSet;
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
    #[serde(default)]
    pub translation_key_types: Vec<String>,
    #[serde(default)]
    pub translation_key_props: Vec<String>,
    #[serde(default)]
    pub unused_keys: bool,
    #[serde(default)]
    pub ignored_scopes: Vec<String>,
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
    #[error("dictionary pattern must be relative to the workspace root")]
    AbsoluteDictionaryPattern,
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("ignored scope {0:?} is not a valid translation key")]
    InvalidIgnoredScope(String),
    #[error("invalid dictionary glob: {0}")]
    Glob(#[from] globset::Error),
    #[error("default locale {0:?} has no matching dictionary")]
    MissingDefaultLocale(String),
    #[error("dictionary {path} is invalid: {message}")]
    InvalidDictionary {
        path: PathBuf,
        message: String,
        line: Option<usize>,
        column: Option<usize>,
    },
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
        for (name, values) in [
            ("translationKeyTypes", &self.translation_key_types),
            ("translationKeyProps", &self.translation_key_props),
        ] {
            if values
                .iter()
                .any(|value| value.is_empty() || value.split('.').any(str::is_empty))
            {
                return Err(ConfigError::Empty(name));
            }
        }
        if let Some(value) = self
            .ignored_scopes
            .iter()
            .find(|value| crate::CanonicalKey::new(*value, &self.key_separator).is_none())
        {
            return Err(ConfigError::InvalidIgnoredScope(value.clone()));
        }
        DictionaryPattern::new(&self.dictionaries)?;
        Ok(())
    }

    pub fn dictionary_pattern(&self) -> Result<DictionaryPattern, ConfigError> {
        DictionaryPattern::new(&self.dictionaries)
    }

    pub fn ignored_scope_set(&self) -> HashSet<String> {
        self.ignored_scopes.iter().cloned().collect()
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
        let normalized = pattern.replace('\\', "/");
        let bytes = normalized.as_bytes();
        let has_windows_drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'/';
        if normalized.starts_with('/') || has_windows_drive {
            return Err(ConfigError::AbsoluteDictionaryPattern);
        }
        let normalized = normalize_pattern(&normalized);
        let (before, after) = normalized.split_once("{locale}").unwrap();
        let glob = format!("{}*{}", before, after);
        Ok(Self {
            before: before.to_owned(),
            after: after.to_owned(),
            matcher: Glob::new(&glob)?.compile_matcher(),
        })
    }

    pub fn locale_for(&self, root: &Path, path: &Path) -> Option<String> {
        let relative = relative_path(root, path)?
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

    pub fn search_root(&self, root: &Path) -> PathBuf {
        let directory = Path::new(&self.before)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        normalize_path(&root.join(directory))
    }
}

fn normalize_pattern(pattern: &str) -> String {
    let mut components: Vec<&str> = Vec::new();
    for component in pattern.split('/') {
        match component {
            "" | "." => {}
            ".." if components.last().is_some_and(|value| *value != "..") => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    components.join("/")
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn relative_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let root = normalize_path(root);
    let path = normalize_path(path);
    let root_components: Vec<_> = root.components().collect();
    let path_components: Vec<_> = path.components().collect();
    let common = root_components
        .iter()
        .zip(&path_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return None;
    }
    let mut relative = PathBuf::new();
    for _ in common..root_components.len() {
        relative.push("..");
    }
    for component in &path_components[common..] {
        relative.push(component.as_os_str());
    }
    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(ignored_scopes: serde_json::Value, separator: &str) -> Config {
        serde_json::from_value(serde_json::json!({
            "dictionaries": "translation.{locale}.json",
            "defaultLocale": "en",
            "keySeparator": separator,
            "scopedFunctions": ["useScopedTranslation"],
            "translationMethods": ["t"],
            "fullKeyFunctions": ["i18next.t"],
            "ignoredScopes": ignored_scopes
        }))
        .unwrap()
    }

    #[test]
    fn validates_and_deduplicates_ignored_scopes() {
        let config = config(serde_json::json!(["Server_Errors", "Server_Errors"]), ".");
        config.validate().unwrap();
        assert_eq!(config.ignored_scope_set().len(), 1);
    }

    #[test]
    fn rejects_invalid_ignored_scopes_for_the_configured_separator() {
        let config = config(serde_json::json!(["Backend////Errors"]), "//");
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidIgnoredScope(value)) if value == "Backend////Errors"
        ));
    }

    #[test]
    fn ignored_scopes_default_to_empty() {
        let config: Config = serde_json::from_value(serde_json::json!({
            "dictionaries": "translation.{locale}.json",
            "defaultLocale": "en",
            "scopedFunctions": ["useScopedTranslation"],
            "translationMethods": ["t"],
            "fullKeyFunctions": ["i18next.t"]
        }))
        .unwrap();
        assert!(config.ignored_scopes.is_empty());
    }

    #[test]
    fn rejects_absolute_dictionary_patterns() {
        for pattern in [
            "/translations/translation.{locale}.json",
            r"C:\translations\translation.{locale}.json",
            r"\\server\share\translation.{locale}.json",
        ] {
            assert!(matches!(
                DictionaryPattern::new(pattern),
                Err(ConfigError::AbsoluteDictionaryPattern)
            ));
        }
    }

    #[test]
    fn matches_a_dictionary_above_the_workspace() {
        let root = Path::new("/project/packages/app");
        let pattern =
            DictionaryPattern::new("../../translations/translation.{locale}.json").unwrap();
        assert_eq!(
            pattern.locale_for(root, Path::new("/project/translations/translation.en.json")),
            Some("en".to_owned())
        );
        assert_eq!(
            pattern.search_root(root),
            Path::new("/project/translations")
        );
    }
}
