use globset::{Glob, GlobMatcher};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    pub dictionaries: String,
    pub default_locale: String,
    #[serde(default)]
    pub default_namespace: Option<String>,
    #[serde(default = "default_separator")]
    pub key_separator: String,
    pub scoped_functions: Vec<ScopedFunctionConfig>,
    pub full_key_functions: Vec<FullKeyFunctionConfig>,
    #[serde(default)]
    pub translation_key_types: Vec<String>,
    #[serde(default)]
    pub translation_key_props: Vec<String>,
    #[serde(default)]
    pub ignored_scopes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedFunctionConfig {
    pub function_name: String,
    #[serde(default)]
    pub default_namespace: Option<String>,
    pub translation_methods: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FullKeyFunctionConfig {
    pub function_name: String,
    #[serde(default)]
    pub default_namespace: Option<String>,
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
    #[error("dictionary pattern must contain at most one {{namespace}} token")]
    NamespaceToken,
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
    #[error("multiple namespaces were found; set defaultNamespace")]
    MissingDefaultNamespace,
    #[error("{field} namespace {namespace:?} has no dictionary for locale {locale:?}")]
    MissingNamespaceDictionary {
        field: String,
        namespace: String,
        locale: String,
    },
    #[error("duplicate function name {name:?} in {field}")]
    DuplicateFunction { field: &'static str, name: String },
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
        if self.dictionaries.matches("{namespace}").count() > 1 {
            return Err(ConfigError::NamespaceToken);
        }
        if self.default_locale.is_empty() {
            return Err(ConfigError::Empty("defaultLocale"));
        }
        if self
            .default_namespace
            .as_ref()
            .is_some_and(String::is_empty)
        {
            return Err(ConfigError::Empty("defaultNamespace"));
        }
        if self.key_separator.is_empty() {
            return Err(ConfigError::Empty("keySeparator"));
        }
        if self.scoped_functions.is_empty() {
            return Err(ConfigError::Empty("scopedFunctions"));
        }
        if self.full_key_functions.is_empty() {
            return Err(ConfigError::Empty("fullKeyFunctions"));
        }
        let mut scoped_names = HashSet::new();
        for function in &self.scoped_functions {
            validate_convention(&function.function_name, "scopedFunctions.functionName")?;
            if !scoped_names.insert(function.function_name.clone()) {
                return Err(ConfigError::DuplicateFunction {
                    field: "scopedFunctions",
                    name: function.function_name.clone(),
                });
            }
            if function.translation_methods.is_empty() {
                return Err(ConfigError::Empty("scopedFunctions.translationMethods"));
            }
            for method in &function.translation_methods {
                validate_convention(method, "scopedFunctions.translationMethods")?;
            }
            validate_optional_namespace(
                function.default_namespace.as_deref(),
                "scopedFunctions.defaultNamespace",
            )?;
        }
        let mut full_key_names = HashSet::new();
        for function in &self.full_key_functions {
            validate_convention(&function.function_name, "fullKeyFunctions.functionName")?;
            if !full_key_names.insert(function.function_name.clone()) {
                return Err(ConfigError::DuplicateFunction {
                    field: "fullKeyFunctions",
                    name: function.function_name.clone(),
                });
            }
            validate_optional_namespace(
                function.default_namespace.as_deref(),
                "fullKeyFunctions.defaultNamespace",
            )?;
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

    pub fn scoped_function(&self, name: &str) -> Option<&ScopedFunctionConfig> {
        self.scoped_functions
            .iter()
            .find(|function| function.function_name == name)
    }

    pub fn full_key_function(&self, name: &str) -> Option<&FullKeyFunctionConfig> {
        self.full_key_functions
            .iter()
            .find(|function| function.function_name == name)
    }
}

fn validate_convention(value: &str, field: &'static str) -> Result<(), ConfigError> {
    if value.is_empty() || value.split('.').any(str::is_empty) {
        Err(ConfigError::Empty(field))
    } else {
        Ok(())
    }
}

fn validate_optional_namespace(
    namespace: Option<&str>,
    field: &'static str,
) -> Result<(), ConfigError> {
    if namespace.is_some_and(str::is_empty) {
        Err(ConfigError::Empty(field))
    } else {
        Ok(())
    }
}

#[derive(Clone)]
pub struct DictionaryPattern {
    pattern: String,
    has_namespace: bool,
    matcher: GlobMatcher,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryIdentity {
    pub locale: String,
    pub namespace: Option<String>,
}

impl DictionaryPattern {
    pub fn new(pattern: &str) -> Result<Self, ConfigError> {
        if pattern.matches("{locale}").count() != 1 {
            return Err(ConfigError::LocaleToken);
        }
        if pattern.matches("{namespace}").count() > 1 {
            return Err(ConfigError::NamespaceToken);
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
        let has_namespace = normalized.contains("{namespace}");
        let glob = normalized
            .replace("{locale}", "*")
            .replace("{namespace}", "*");
        Ok(Self {
            pattern: normalized,
            has_namespace,
            matcher: Glob::new(&glob)?.compile_matcher(),
        })
    }

    pub fn identity_for(&self, root: &Path, path: &Path) -> Option<DictionaryIdentity> {
        let relative = relative_path(root, path)?
            .to_string_lossy()
            .replace('\\', "/");
        if !self.matcher.is_match(&relative) {
            return None;
        }
        let mut captures = HashMap::new();
        if !capture_tokens(&self.pattern, &relative, &mut captures) {
            return None;
        }
        Some(DictionaryIdentity {
            locale: captures.remove("locale")?,
            namespace: captures.remove("namespace"),
        })
    }

    pub fn locale_for(&self, root: &Path, path: &Path) -> Option<String> {
        self.identity_for(root, path)
            .map(|identity| identity.locale)
    }

    pub fn has_namespace(&self) -> bool {
        self.has_namespace
    }

    pub fn search_root(&self, root: &Path) -> PathBuf {
        let literal_prefix = self
            .pattern
            .find('{')
            .map_or(self.pattern.as_str(), |index| &self.pattern[..index]);
        let directory = Path::new(literal_prefix)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        normalize_path(&root.join(directory))
    }
}

fn capture_tokens(pattern: &str, value: &str, out: &mut HashMap<String, String>) -> bool {
    let mut pattern_at = 0;
    let mut value_at = 0;
    while let Some(open_rel) = pattern[pattern_at..].find('{') {
        let open = pattern_at + open_rel;
        let Some(close_rel) = pattern[open..].find('}') else {
            return false;
        };
        let close = open + close_rel;
        let literal = &pattern[pattern_at..open];
        if !value[value_at..].starts_with(literal) {
            return false;
        }
        value_at += literal.len();
        let token = &pattern[open + 1..close];
        let next_pattern = close + 1;
        let next_literal_end = pattern[next_pattern..]
            .find('{')
            .map_or(pattern.len(), |i| next_pattern + i);
        let next_literal = &pattern[next_pattern..next_literal_end];
        let capture_end = if next_literal.is_empty() {
            value.len()
        } else {
            value[value_at..]
                .find(next_literal)
                .map(|i| value_at + i)
                .unwrap_or(value.len())
        };
        let capture = &value[value_at..capture_end];
        if capture.is_empty()
            || capture.contains('/')
            || out.insert(token.to_owned(), capture.to_owned()).is_some()
        {
            return false;
        }
        value_at = capture_end;
        pattern_at = next_pattern;
    }
    value[value_at..] == pattern[pattern_at..]
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
            "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}],
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
            "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
        }))
        .unwrap();
        assert!(config.ignored_scopes.is_empty());
    }

    #[test]
    fn rejects_removed_unused_keys_setting() {
        let result = serde_json::from_value::<Config>(serde_json::json!({
            "dictionaries": "translation.{locale}.json",
            "defaultLocale": "en",
            "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}],
            "unusedKeys": true
        }));
        assert!(result.is_err());
    }

    #[test]
    fn rejects_legacy_function_arrays_and_top_level_methods() {
        let legacy = serde_json::from_value::<Config>(serde_json::json!({
            "dictionaries": "translation.{locale}.json",
            "defaultLocale": "en",
            "scopedFunctions": ["useScopedTranslation"],
            "translationMethods": ["t"],
            "fullKeyFunctions": ["translate"]
        }));
        assert!(legacy.is_err());
    }

    #[test]
    fn validates_function_entries_and_rejects_duplicate_names() {
        let config: Config = serde_json::from_value(serde_json::json!({
            "dictionaries": "locales/{locale}/{namespace}.json",
            "defaultLocale": "en",
            "defaultNamespace": "common",
            "scopedFunctions": [
                {"functionName":"useA","translationMethods":["t"]},
                {"functionName":"useA","defaultNamespace":"home","translationMethods":["key"]}
            ],
            "fullKeyFunctions": [{"functionName":"translate","defaultNamespace":"home"}]
        }))
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::DuplicateFunction { field: "scopedFunctions", name }) if name == "useA"
        ));
    }

    #[test]
    fn rejects_empty_scoped_translation_methods() {
        let config: Config = serde_json::from_value(serde_json::json!({
            "dictionaries": "translation.{locale}.json",
            "defaultLocale": "en",
            "scopedFunctions": [{"functionName":"useScopedTranslation","translationMethods":[]}],
            "fullKeyFunctions": [{"functionName":"translate"}]
        }))
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::Empty("scopedFunctions.translationMethods"))
        ));
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

    #[test]
    fn extracts_locale_and_namespace_in_either_order() {
        let root = Path::new("/project");
        let pattern = DictionaryPattern::new("locales/{locale}/{namespace}.json").unwrap();
        assert_eq!(
            pattern.identity_for(root, Path::new("/project/locales/en/common.json")),
            Some(DictionaryIdentity {
                locale: "en".into(),
                namespace: Some("common".into())
            })
        );
        assert!(pattern.has_namespace());
    }

    #[test]
    fn rejects_duplicate_namespace_tokens() {
        assert!(matches!(
            DictionaryPattern::new("{namespace}/{locale}/{namespace}.json"),
            Err(ConfigError::NamespaceToken)
        ));
    }
}
