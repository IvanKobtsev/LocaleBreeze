use crate::{
    CanonicalKey, Config, DictionaryEntry, EntryKind, FileContribution, OccurrenceKind,
    SourceOccurrence, analyze_source, parse_dictionary,
};
use arc_swap::ArcSwap;
use ignore::WalkBuilder;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

#[derive(Clone, Debug)]
pub enum CompletionContext {
    Scope { query: String },
    FullKey { query: String },
    ScopedKey { scope: CanonicalKey, query: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionCandidate {
    pub key: String,
    pub canonical_key: String,
    pub detail: Option<String>,
    pub score: i64,
}

#[derive(Clone, Default)]
pub struct IndexSnapshot {
    pub generation: u64,
    pub files: HashMap<Url, Arc<FileContribution>>,
    dictionaries: BTreeMap<CanonicalKey, Vec<DictionaryEntry>>,
    occurrences: BTreeMap<CanonicalKey, Vec<SourceOccurrence>>,
}

impl IndexSnapshot {
    fn rebuild(generation: u64, files: HashMap<Url, Arc<FileContribution>>) -> Self {
        let mut dictionaries: BTreeMap<CanonicalKey, Vec<DictionaryEntry>> = BTreeMap::new();
        let mut occurrences: BTreeMap<CanonicalKey, Vec<SourceOccurrence>> = BTreeMap::new();
        for file in files.values() {
            for entry in &file.dictionaries {
                dictionaries
                    .entry(entry.key.clone())
                    .or_default()
                    .push(entry.clone());
            }
            for occurrence in &file.occurrences {
                occurrences
                    .entry(occurrence.key.clone())
                    .or_default()
                    .push(occurrence.clone());
            }
        }
        for values in dictionaries.values_mut() {
            values.sort_by(|a, b| {
                a.locale
                    .cmp(&b.locale)
                    .then(a.uri.as_str().cmp(b.uri.as_str()))
            });
        }
        Self {
            generation,
            files,
            dictionaries,
            occurrences,
        }
    }

    pub fn dictionary_entries(&self, key: &CanonicalKey) -> &[DictionaryEntry] {
        self.dictionaries
            .get(key)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn occurrences(&self, key: &CanonicalKey) -> &[SourceOccurrence] {
        self.occurrences
            .get(key)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn occurrence_at(&self, uri: &Url, offset: usize) -> Option<&SourceOccurrence> {
        self.files
            .get(uri)?
            .occurrences
            .iter()
            .find(|o| o.range.contains(offset))
    }

    pub fn dictionary_at(&self, uri: &Url, offset: usize) -> Option<&DictionaryEntry> {
        self.files
            .get(uri)?
            .dictionaries
            .iter()
            .filter(|e| e.key_range.contains(offset))
            .max_by_key(|e| e.key.as_str().len())
    }

    pub fn text(&self, uri: &Url) -> Option<&str> {
        Some(&self.files.get(uri)?.text)
    }

    pub fn completion_context_at(
        &self,
        uri: &Url,
        offset: usize,
        config: &Config,
    ) -> Option<CompletionContext> {
        let file = self.files.get(uri)?;
        let before = file.text.get(..offset)?;
        let quote_at = before.rfind(['\'', '"'])?;
        let query = before.get(quote_at + 1..)?.to_owned();
        if query.contains(['\n', '\r', '\'', '"']) {
            return None;
        }
        let call_prefix = before.get(..quote_at)?.trim_end();
        let open = call_prefix.rfind('(')?;
        let callee = call_prefix.get(..open)?.trim_end();
        let callee = callee
            .rsplit(|c: char| c.is_whitespace() || matches!(c, ';' | '=' | '{' | '}'))
            .next()?;
        if config.scoped_functions.iter().any(|x| x == callee) {
            return Some(CompletionContext::Scope { query });
        }
        if config.full_key_functions.iter().any(|x| x == callee) {
            return Some(CompletionContext::FullKey { query });
        }
        let binding = file.bindings.iter().rev().find(|b| {
            b.visibility.contains(offset)
                && if b.direct_function {
                    callee == b.name
                } else {
                    callee == format!("{}.{}", b.name, b.method)
                }
        })?;
        Some(CompletionContext::ScopedKey {
            scope: binding.scope.clone(),
            query,
        })
    }

    pub fn completions(
        &self,
        context: &CompletionContext,
        default_locale: &str,
        separator: &str,
        limit: usize,
    ) -> (Vec<CompletionCandidate>, bool) {
        let (query, scope, objects) = match context {
            CompletionContext::Scope { query } => (query.as_str(), None, true),
            CompletionContext::FullKey { query } => (query.as_str(), None, false),
            CompletionContext::ScopedKey { scope, query } => (query.as_str(), Some(scope), false),
        };
        let normalized_query = normalize(query);
        let mut candidates = Vec::new();
        for (key, entries) in &self.dictionaries {
            let is_object = entries.iter().any(|e| e.kind == EntryKind::Object);
            if objects != is_object {
                continue;
            }
            let insert = if let Some(scope) = scope {
                let Some(relative) = key.relative_to(scope, separator) else {
                    continue;
                };
                relative.to_owned()
            } else {
                key.as_str().to_owned()
            };
            if !objects && entries.iter().all(|e| e.kind != EntryKind::Leaf) {
                continue;
            }
            let default_value = entries
                .iter()
                .find(|e| e.locale == default_locale)
                .and_then(|e| e.value.as_deref());
            let mut haystacks = vec![normalize(key.as_str()), normalize(&insert)];
            haystacks.extend(
                entries
                    .iter()
                    .filter_map(|e| e.value.as_deref())
                    .map(normalize),
            );
            if objects {
                for (descendant, values) in self.dictionaries.range(key.clone()..) {
                    if descendant == key
                        || !descendant
                            .as_str()
                            .starts_with(&format!("{}{}", key, separator))
                    {
                        if descendant > key {
                            break;
                        } else {
                            continue;
                        }
                    }
                    haystacks.push(normalize(descendant.as_str()));
                    haystacks.extend(
                        values
                            .iter()
                            .filter_map(|e| e.value.as_deref())
                            .map(normalize),
                    );
                }
            }
            let Some(score) = haystacks
                .iter()
                .filter_map(|h| match_score(&normalized_query, h, &normalize(&insert)))
                .max()
            else {
                continue;
            };
            candidates.push(CompletionCandidate {
                key: insert,
                canonical_key: key.as_str().into(),
                detail: default_value.map(str::to_owned),
                score,
            });
        }
        candidates.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.key.cmp(&b.key)));
        let incomplete = candidates.len() > limit;
        candidates.truncate(limit);
        (candidates, incomplete)
    }

    pub fn immediate_child_occurrences(
        &self,
        scope: &CanonicalKey,
        separator: &str,
    ) -> Vec<&SourceOccurrence> {
        self.occurrences
            .values()
            .flatten()
            .filter(|o| {
                o.kind == OccurrenceKind::ScopeDeclaration && &o.key == scope
                    || o.key.parent(separator).as_ref() == Some(scope)
            })
            .collect()
    }
}

fn normalize(value: &str) -> String {
    value.to_lowercase()
}

fn match_score(query: &str, haystack: &str, insert: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(1);
    }
    if insert == query {
        return Some(10_000);
    }
    if insert.starts_with(query) {
        return Some(9_000 - insert.len() as i64);
    }
    if insert.split(['.', '_', '-']).any(|s| s.starts_with(query)) {
        return Some(8_000 - insert.len() as i64);
    }
    if let Some(index) = haystack.find(query) {
        return Some(7_000 - index as i64 - haystack.len() as i64 / 10);
    }
    let mut at = 0usize;
    for ch in query.chars() {
        let found = haystack[at..].find(ch)?;
        at += found + ch.len_utf8();
    }
    Some(5_000 - at as i64)
}

pub struct WorkspaceIndex {
    root: PathBuf,
    config: Config,
    snapshot: ArcSwap<IndexSnapshot>,
}

impl WorkspaceIndex {
    pub fn load(root: PathBuf, config_path: &Path) -> Result<Self, crate::ConfigError> {
        let config = Config::load(config_path)?;
        let this = Self {
            root,
            config,
            snapshot: ArcSwap::from_pointee(IndexSnapshot::default()),
        };
        this.rescan();
        if !this
            .snapshot()
            .dictionaries
            .values()
            .flatten()
            .any(|entry| entry.locale == this.config.default_locale)
        {
            return Err(crate::ConfigError::MissingDefaultLocale(
                this.config.default_locale.clone(),
            ));
        }
        Ok(this)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn snapshot(&self) -> Arc<IndexSnapshot> {
        self.snapshot.load_full()
    }

    pub fn rescan(&self) {
        let pattern = self.config.dictionary_pattern().expect("validated pattern");
        let mut files = HashMap::new();
        for result in WalkBuilder::new(&self.root).standard_filters(true).build() {
            let Ok(entry) = result else { continue };
            if !entry.file_type().is_some_and(|x| x.is_file()) {
                continue;
            }
            let path = entry.path();
            let Some(contribution) = self.parse_disk_file(path, &pattern) else {
                continue;
            };
            files.insert(contribution.uri.clone(), Arc::new(contribution));
        }
        let generation = self.snapshot.load().generation + 1;
        self.snapshot
            .store(Arc::new(IndexSnapshot::rebuild(generation, files)));
    }

    pub fn update_text(&self, uri: Url, text: String, version: Option<i32>) {
        if let Some(current) = self.snapshot.load().files.get(&uri)
            && current
                .version
                .zip(version)
                .is_some_and(|(old, new)| old >= new)
        {
            return;
        }
        let contribution = self.parse_text(uri.clone(), text, version);
        let current = self.snapshot.load_full();
        let mut files = current.files.clone();
        if let Some(contribution) = contribution {
            files.insert(uri, Arc::new(contribution));
        } else {
            files.remove(&uri);
        }
        self.snapshot.store(Arc::new(IndexSnapshot::rebuild(
            current.generation + 1,
            files,
        )));
    }

    pub fn close_document(&self, uri: &Url) {
        if let Ok(path) = uri.to_file_path() {
            let pattern = self.config.dictionary_pattern().expect("validated pattern");
            let current = self.snapshot.load_full();
            let mut files = current.files.clone();
            if let Some(contribution) = self.parse_disk_file(&path, &pattern) {
                files.insert(uri.clone(), Arc::new(contribution));
            } else {
                files.remove(uri);
            }
            self.snapshot.store(Arc::new(IndexSnapshot::rebuild(
                current.generation + 1,
                files,
            )));
        }
    }

    pub fn refresh_disk_path(&self, path: &Path) {
        let Ok(uri) = Url::from_file_path(path) else {
            return;
        };
        let current = self.snapshot.load_full();
        if current
            .files
            .get(&uri)
            .is_some_and(|file| file.version.is_some())
        {
            return;
        }
        let pattern = self.config.dictionary_pattern().expect("validated pattern");
        let mut files = current.files.clone();
        if let Some(contribution) = self.parse_disk_file(path, &pattern) {
            files.insert(uri, Arc::new(contribution));
        } else {
            files.remove(&uri);
        }
        self.snapshot.store(Arc::new(IndexSnapshot::rebuild(
            current.generation + 1,
            files,
        )));
    }

    fn parse_disk_file(
        &self,
        path: &Path,
        pattern: &crate::DictionaryPattern,
    ) -> Option<FileContribution> {
        let text = std::fs::read_to_string(path).ok()?;
        let uri = Url::from_file_path(path).ok()?;
        if let Some(locale) = pattern.locale_for(&self.root, path) {
            let dictionaries =
                parse_dictionary(&uri, &locale, &text, &self.config.key_separator).ok()?;
            Some(FileContribution {
                uri,
                text,
                version: None,
                dictionaries,
                occurrences: vec![],
                bindings: vec![],
            })
        } else if is_source(path) {
            self.parse_text(uri, text, None)
        } else {
            None
        }
    }

    fn parse_text(&self, uri: Url, text: String, version: Option<i32>) -> Option<FileContribution> {
        let path = uri.to_file_path().ok()?;
        let pattern = self.config.dictionary_pattern().ok()?;
        if let Some(locale) = pattern.locale_for(&self.root, &path) {
            let dictionaries =
                parse_dictionary(&uri, &locale, &text, &self.config.key_separator).ok()?;
            Some(FileContribution {
                uri,
                text,
                version,
                dictionaries,
                occurrences: vec![],
                bindings: vec![],
            })
        } else if is_source(&path) {
            let (occurrences, bindings) = analyze_source(
                &uri,
                &text,
                &self.config.key_separator,
                &self.config.scoped_functions,
                &self.config.translation_methods,
                &self.config.full_key_functions,
            );
            Some(FileContribution {
                uri,
                text,
                version,
                dictionaries: vec![],
                occurrences,
                bindings,
            })
        } else {
            None
        }
    }
}

fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| matches!(x, "js" | "jsx" | "ts" | "tsx"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn searches_values_and_relative_keys() {
        let uri = Url::parse("file:///translation.en.json").unwrap();
        let text = r#"{"Page":{"Login":{"my_key":"My value"}}}"#.to_string();
        let dictionaries = parse_dictionary(&uri, "en", &text, ".").unwrap();
        let contribution = FileContribution {
            uri: uri.clone(),
            text,
            version: None,
            dictionaries,
            occurrences: vec![],
            bindings: vec![],
        };
        let snapshot = IndexSnapshot::rebuild(1, HashMap::from([(uri, Arc::new(contribution))]));
        let scope = CanonicalKey::new("Page.Login", ".").unwrap();
        for query in ["my_k", "My value"] {
            let (found, _) = snapshot.completions(
                &CompletionContext::ScopedKey {
                    scope: scope.clone(),
                    query: query.into(),
                },
                "en",
                ".",
                20,
            );
            assert_eq!(found[0].key, "my_key");
        }
    }

    #[test]
    fn detects_completion_in_empty_literal() {
        let uri = Url::parse("file:///app.tsx").unwrap();
        let text = "const i18n=useScopedTranslation('Page.Login'); i18n.t('')".to_string();
        let (occurrences, bindings) = analyze_source(
            &uri,
            &text,
            ".",
            &["useScopedTranslation".into()],
            &["t".into()],
            &["i18next.t".into()],
        );
        let contribution = FileContribution {
            uri: uri.clone(),
            text: text.clone(),
            version: None,
            dictionaries: vec![],
            occurrences,
            bindings,
        };
        let snapshot =
            IndexSnapshot::rebuild(1, HashMap::from([(uri.clone(), Arc::new(contribution))]));
        let config: Config = serde_json::from_value(serde_json::json!({
            "dictionaries":"translation.{locale}.json", "defaultLocale":"en", "keySeparator":".",
            "scopedFunctions":["useScopedTranslation"], "translationMethods":["t"], "fullKeyFunctions":["i18next.t"]
        })).unwrap();
        assert!(matches!(
            snapshot.completion_context_at(&uri, text.len() - 1, &config),
            Some(CompletionContext::ScopedKey { .. })
        ));
    }

    #[test]
    fn loads_and_incrementally_replaces_a_workspace() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("dict")).unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"dict/translation.{locale}.json",
              "defaultLocale":"en",
              "keySeparator":".",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"]
            }"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("dict/translation.en.json"),
            r#"{"Page":{"Login":{"submit":"Sign in"}}}"#,
        )
        .unwrap();
        let source_path = temp.path().join("app.tsx");
        std::fs::write(
            &source_path,
            "const i18n=useScopedTranslation('Page.Login'); i18n.t('submit')",
        )
        .unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let key = CanonicalKey::new("Page.Login.submit", ".").unwrap();
        assert_eq!(workspace.snapshot().occurrences(&key).len(), 1);
        let uri = Url::from_file_path(source_path).unwrap();
        workspace.update_text(uri, "i18next.t('Page.Login.cancel')".into(), Some(1));
        assert!(workspace.snapshot().occurrences(&key).is_empty());
    }
}
