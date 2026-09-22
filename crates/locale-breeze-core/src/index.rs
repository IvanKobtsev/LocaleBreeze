use crate::{
    CanonicalKey, Config, DictionaryEntry, EntryKind, FileContribution, OccurrenceKind,
    SourceOccurrence, analyze_source, parse_dictionary_ignoring,
};
use arc_swap::ArcSwap;
use ignore::WalkBuilder;
use std::collections::{BTreeMap, HashMap, HashSet};
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
        let files = files
            .into_values()
            .map(|file| (normalized_uri(&file.uri), file))
            .collect::<HashMap<_, _>>();
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

    pub fn direct_dictionary_children<'a>(
        &'a self,
        scope: &'a CanonicalKey,
        separator: &'a str,
    ) -> impl Iterator<Item = &'a DictionaryEntry> + 'a {
        self.dictionaries
            .iter()
            .filter(move |(key, _)| key.parent(separator).as_ref() == Some(scope))
            .flat_map(|(_, entries)| entries)
    }

    pub fn occurrences(&self, key: &CanonicalKey) -> &[SourceOccurrence] {
        self.occurrences
            .get(key)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn default_locale_leaf_entries<'a>(
        &'a self,
        default_locale: &'a str,
    ) -> impl Iterator<Item = &'a DictionaryEntry> + 'a {
        self.dictionaries
            .values()
            .flatten()
            .filter(move |entry| entry.locale == default_locale && entry.kind == EntryKind::Leaf)
    }

    pub fn is_leaf_key_used(&self, key: &CanonicalKey) -> bool {
        self.occurrences(key)
            .iter()
            .any(|occurrence| occurrence.kind != OccurrenceKind::ScopeDeclaration)
            || self.dynamic_scope_occurrences(key, ".").next().is_some()
    }

    pub fn is_leaf_key_used_with_separator(&self, key: &CanonicalKey, separator: &str) -> bool {
        self.occurrences(key)
            .iter()
            .any(|occurrence| occurrence.kind != OccurrenceKind::ScopeDeclaration)
            || self
                .dynamic_scope_occurrences(key, separator)
                .next()
                .is_some()
    }

    pub fn dynamic_scope_occurrences<'a>(
        &'a self,
        key: &'a CanonicalKey,
        separator: &'a str,
    ) -> impl Iterator<Item = &'a SourceOccurrence> + 'a {
        self.occurrences
            .values()
            .flatten()
            .filter(move |occurrence| {
                occurrence.kind == OccurrenceKind::DynamicScope
                    && (occurrence.key == *key
                        || key
                            .as_str()
                            .starts_with(&format!("{}{}", occurrence.key, separator)))
            })
    }

    pub fn occurrence_at(&self, uri: &Url, offset: usize) -> Option<&SourceOccurrence> {
        self.file(uri)?
            .occurrences
            .iter()
            .find(|o| o.range.contains(offset))
    }

    pub fn dictionary_at(&self, uri: &Url, offset: usize) -> Option<&DictionaryEntry> {
        self.file(uri)?
            .dictionaries
            .iter()
            .filter(|e| e.key_range.contains(offset))
            .max_by_key(|e| e.key.as_str().len())
    }

    pub fn text(&self, uri: &Url) -> Option<&str> {
        Some(&self.file(uri)?.text)
    }

    pub fn version(&self, uri: &Url) -> Option<i32> {
        self.file(uri)?.version
    }

    pub fn source_occurrences(&self, uri: &Url) -> &[SourceOccurrence] {
        self.file(uri)
            .map(|file| file.occurrences.as_slice())
            .unwrap_or_default()
    }

    pub fn ignored_occurrences(&self) -> impl Iterator<Item = &SourceOccurrence> {
        self.files
            .values()
            .flat_map(|file| file.ignored_occurrences.iter())
    }

    pub fn dictionary_entries_all(&self) -> impl Iterator<Item = &DictionaryEntry> {
        self.dictionaries.values().flatten()
    }

    fn file(&self, uri: &Url) -> Option<&FileContribution> {
        self.files.get(&normalized_uri(uri)).map(AsRef::as_ref)
    }

    pub fn completion_context_at(
        &self,
        uri: &Url,
        offset: usize,
        config: &Config,
    ) -> Option<CompletionContext> {
        let file = self.file(uri)?;
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
            if objects {
                for (descendant, _) in self.dictionaries.range(key.clone()..) {
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

    pub fn scope_occurrences(
        &self,
        scope: &CanonicalKey,
        separator: &str,
        recursive_leaf_limit: usize,
    ) -> Vec<&SourceOccurrence> {
        let descendant_prefix = format!("{}{}", scope, separator);
        let descendant_leaf_count = self
            .dictionaries
            .range(scope.clone()..)
            .filter(|(key, entries)| {
                key.as_str().starts_with(&descendant_prefix)
                    && entries.iter().any(|entry| entry.kind == EntryKind::Leaf)
            })
            .take(recursive_leaf_limit + 1)
            .count();
        let recursive = descendant_leaf_count <= recursive_leaf_limit;

        self.occurrences
            .values()
            .flatten()
            .filter(|o| {
                o.kind == OccurrenceKind::ScopeDeclaration && &o.key == scope
                    || o.kind == OccurrenceKind::DynamicScope
                        && (o.key == *scope
                            || scope
                                .as_str()
                                .starts_with(&format!("{}{}", o.key, separator)))
                    || if recursive {
                        o.key.as_str().starts_with(&descendant_prefix)
                    } else {
                        o.key.parent(separator).as_ref() == Some(scope)
                    }
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
    ignored_scopes: HashSet<String>,
    snapshot: ArcSwap<IndexSnapshot>,
}

#[derive(Clone, Debug)]
pub struct DictionaryIssue {
    pub path: PathBuf,
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl WorkspaceIndex {
    pub fn load(root: PathBuf, config_path: &Path) -> Result<Self, crate::ConfigError> {
        Self::load_with_unused_override(root, config_path, None)
    }

    pub fn load_with_unused_override(
        root: PathBuf,
        config_path: &Path,
        unused_keys_override: Option<bool>,
    ) -> Result<Self, crate::ConfigError> {
        let mut config = Config::load(config_path)?;
        if let Some(value) = unused_keys_override {
            config.unused_keys = value;
        }
        let ignored_scopes = config.ignored_scope_set();
        let this = Self {
            root,
            config,
            ignored_scopes,
            snapshot: ArcSwap::from_pointee(IndexSnapshot::default()),
        };
        this.rescan();
        let dictionary_pattern = this.config.dictionary_pattern()?;
        if let Some(issue) = this.first_dictionary_issue(&dictionary_pattern) {
            return Err(crate::ConfigError::InvalidDictionary {
                path: issue.path,
                message: issue.message,
                line: issue.line,
                column: issue.column,
            });
        }
        if !this
            .snapshot()
            .files
            .values()
            .filter_map(|file| file.uri.to_file_path().ok())
            .filter_map(|path| dictionary_pattern.locale_for(&this.root, &path))
            .any(|locale| locale == this.config.default_locale)
        {
            return Err(crate::ConfigError::MissingDefaultLocale(
                this.config.default_locale.clone(),
            ));
        }
        Ok(this)
    }

    fn first_dictionary_issue(
        &self,
        pattern: &crate::DictionaryPattern,
    ) -> Option<DictionaryIssue> {
        let root = pattern.search_root(&self.root);
        for result in WalkBuilder::new(root).standard_filters(true).build() {
            let entry = result.ok()?;
            if !entry.file_type().is_some_and(|kind| kind.is_file())
                || pattern.locale_for(&self.root, entry.path()).is_none()
            {
                continue;
            }
            let path = entry.path();
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    return Some(DictionaryIssue {
                        path: path.to_owned(),
                        message: format!("could not read dictionary: {error}"),
                        line: None,
                        column: None,
                    });
                }
            };
            let uri = Url::from_file_path(path).ok()?;
            let locale = pattern.locale_for(&self.root, path)?;
            if let Err(error) = parse_dictionary_ignoring(
                &uri,
                &locale,
                &text,
                &self.config.key_separator,
                &|key| self.is_ignored_key(key),
            ) {
                let (line, column) = match &error {
                    crate::DictionaryError::InvalidJson(source) => {
                        (Some(source.line()), Some(source.column()))
                    }
                    crate::DictionaryError::Parser => (None, None),
                };
                return Some(DictionaryIssue {
                    path: path.to_owned(),
                    message: error.to_string(),
                    line,
                    column,
                });
            }
        }
        None
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn dictionary_root(&self) -> PathBuf {
        self.config
            .dictionary_pattern()
            .expect("validated pattern")
            .search_root(&self.root)
    }
    pub fn is_dictionary_path(&self, path: &Path) -> bool {
        self.config
            .dictionary_pattern()
            .ok()
            .and_then(|pattern| pattern.locale_for(&self.root, path))
            .is_some()
    }
    pub fn contains_path(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
            || self
                .config
                .dictionary_pattern()
                .expect("validated pattern")
                .locale_for(&self.root, path)
                .is_some()
    }
    pub fn is_ignored_key(&self, key: &CanonicalKey) -> bool {
        is_ignored_key(
            &self.ignored_scopes,
            key.as_str(),
            &self.config.key_separator,
        )
    }
    pub fn snapshot(&self) -> Arc<IndexSnapshot> {
        self.snapshot.load_full()
    }

    pub fn rescan(&self) {
        let pattern = self.config.dictionary_pattern().expect("validated pattern");
        let mut files = HashMap::new();
        let dictionary_root = pattern.search_root(&self.root);
        let mut scan_roots = vec![self.root.clone()];
        if !dictionary_root.starts_with(&self.root) {
            scan_roots.push(dictionary_root);
        }
        for scan_root in &scan_roots {
            for result in WalkBuilder::new(scan_root).standard_filters(true).build() {
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
        }
        let generation = self.snapshot.load().generation + 1;
        self.snapshot
            .store(Arc::new(IndexSnapshot::rebuild(generation, files)));
    }

    pub fn update_text(&self, uri: Url, text: String, version: Option<i32>) {
        let file_key = normalized_uri(&uri);
        let current = self.snapshot.load_full();
        let current_file = current.files.get(&file_key);
        if let Some(current) = current_file
            && current
                .version
                .zip(version)
                .is_some_and(|(old, new)| old >= new)
        {
            return;
        }
        // Keep the URI that originally identified the indexed file. Some LSP
        // clients send the same Windows path later with a lower-case drive and
        // an encoded colon (`file:///c%3A/...`). Letting that spelling replace
        // the disk URI makes clients such as WebStorm retain two diagnostics
        // collections for the same physical file.
        let uri = current_file
            .map(|file| file.uri.clone())
            .unwrap_or_else(|| canonical_file_uri(&uri));
        let contribution = self.parse_text(uri.clone(), text.clone(), version);
        let mut files = current.files.clone();
        if let Some(contribution) = contribution {
            files.insert(file_key.clone(), Arc::new(contribution));
        } else {
            // Keep the latest document text even when an intermediate edit is
            // not parseable. LSP changes are incremental, so dropping the file
            // here would leave no base text to which the next change can apply.
            files.insert(
                file_key.clone(),
                Arc::new(FileContribution {
                    uri,
                    text,
                    version,
                    dictionaries: vec![],
                    occurrences: vec![],
                    ignored_occurrences: vec![],
                    bindings: vec![],
                }),
            );
        }
        self.snapshot.store(Arc::new(IndexSnapshot::rebuild(
            current.generation + 1,
            files,
        )));
    }

    pub fn close_document(&self, uri: &Url) {
        if let Ok(path) = uri.to_file_path() {
            let file_key = normalized_uri(uri);
            let pattern = self.config.dictionary_pattern().expect("validated pattern");
            let current = self.snapshot.load_full();
            let mut files = current.files.clone();
            if let Some(contribution) = self.parse_disk_file(&path, &pattern) {
                files.insert(file_key.clone(), Arc::new(contribution));
            } else {
                files.remove(&file_key);
            }
            self.snapshot.store(Arc::new(IndexSnapshot::rebuild(
                current.generation + 1,
                files,
            )));
        }
    }

    pub fn refresh_disk_path(&self, path: &Path) -> Option<DictionaryIssue> {
        let Ok(uri) = Url::from_file_path(path) else {
            return None;
        };
        let file_key = normalized_uri(&uri);
        let current = self.snapshot.load_full();
        let pattern = self.config.dictionary_pattern().expect("validated pattern");
        let dictionary_issue = pattern.locale_for(&self.root, path).and_then(|locale| {
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    return Some(DictionaryIssue {
                        path: path.to_owned(),
                        message: format!("could not read dictionary: {error}"),
                        line: None,
                        column: None,
                    });
                }
            };
            parse_dictionary_ignoring(&uri, &locale, &text, &self.config.key_separator, &|key| {
                self.is_ignored_key(key)
            })
            .err()
            .map(|error| {
                let (line, column) = match &error {
                    crate::DictionaryError::InvalidJson(source) => {
                        (Some(source.line()), Some(source.column()))
                    }
                    crate::DictionaryError::Parser => (None, None),
                };
                DictionaryIssue {
                    path: path.to_owned(),
                    message: error.to_string(),
                    line,
                    column,
                }
            })
        });
        let mut files = current.files.clone();
        if let Some(contribution) = self.parse_disk_file(path, &pattern) {
            files.insert(file_key.clone(), Arc::new(contribution));
        } else {
            files.remove(&file_key);
        }
        self.snapshot.store(Arc::new(IndexSnapshot::rebuild(
            current.generation + 1,
            files,
        )));
        dictionary_issue
    }

    fn parse_disk_file(
        &self,
        path: &Path,
        pattern: &crate::DictionaryPattern,
    ) -> Option<FileContribution> {
        let text = std::fs::read_to_string(path).ok()?;
        let uri = Url::from_file_path(path).ok()?;
        if let Some(locale) = pattern.locale_for(&self.root, path) {
            let dictionaries = parse_dictionary_ignoring(
                &uri,
                &locale,
                &text,
                &self.config.key_separator,
                &|key| self.is_ignored_key(key),
            )
            .ok()?;
            Some(FileContribution {
                uri,
                text,
                version: None,
                dictionaries,
                occurrences: vec![],
                ignored_occurrences: vec![],
                bindings: vec![],
            })
        } else if path.starts_with(&self.root) && is_source(path) {
            self.parse_text(uri, text, None)
        } else {
            None
        }
    }

    fn parse_text(&self, uri: Url, text: String, version: Option<i32>) -> Option<FileContribution> {
        let path = uri.to_file_path().ok()?;
        let pattern = self.config.dictionary_pattern().ok()?;
        if let Some(locale) = pattern.locale_for(&self.root, &path) {
            let dictionaries = parse_dictionary_ignoring(
                &uri,
                &locale,
                &text,
                &self.config.key_separator,
                &|key| self.is_ignored_key(key),
            )
            .ok()?;
            Some(FileContribution {
                uri,
                text,
                version,
                dictionaries,
                occurrences: vec![],
                ignored_occurrences: vec![],
                bindings: vec![],
            })
        } else if is_source(&path) {
            let (analyzed_occurrences, bindings) = analyze_source(
                &uri,
                &text,
                &self.config.key_separator,
                &self.config.scoped_functions,
                &self.config.translation_methods,
                &self.config.full_key_functions,
                &self.config.translation_key_types,
                &self.config.translation_key_props,
            );
            let mut occurrences = Vec::new();
            let mut ignored_occurrences = Vec::new();
            for occurrence in analyzed_occurrences {
                if !self.is_ignored_key(&occurrence.key) {
                    occurrences.push(occurrence);
                } else if !occurrence.range.0.is_empty()
                    && (occurrence.kind == OccurrenceKind::ScopeDeclaration
                        || occurrence
                            .scope
                            .as_ref()
                            .is_none_or(|scope| !self.is_ignored_key(scope)))
                {
                    ignored_occurrences.push(occurrence);
                }
            }
            Some(FileContribution {
                uri,
                text,
                version,
                dictionaries: vec![],
                occurrences,
                ignored_occurrences,
                bindings,
            })
        } else {
            None
        }
    }
}

fn is_ignored_key(ignored_scopes: &HashSet<String>, key: &str, separator: &str) -> bool {
    ignored_scopes.contains(key)
        || key
            .match_indices(separator)
            .any(|(index, _)| ignored_scopes.contains(&key[..index]))
}

fn normalized_uri(uri: &Url) -> Url {
    #[cfg(windows)]
    {
        if uri.scheme() == "file" {
            let canonical = canonical_file_uri(uri);
            return Url::parse(&canonical.as_str().to_ascii_lowercase()).unwrap_or(canonical);
        }
    }
    uri.clone()
}

fn canonical_file_uri(uri: &Url) -> Url {
    if uri.scheme() == "file"
        && let Ok(path) = uri.to_file_path()
        && let Ok(canonical) = Url::from_file_path(path)
    {
        return canonical;
    }
    uri.clone()
}

fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| matches!(x, "js" | "jsx" | "ts" | "tsx"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_dictionary;

    #[test]
    fn ignored_scope_matching_is_exact_and_separator_aware() {
        let ignored = HashSet::from(["Server_Errors".to_owned(), "Backend//Validation".to_owned()]);
        assert!(is_ignored_key(&ignored, "Server_Errors", "."));
        assert!(is_ignored_key(&ignored, "Server_Errors.InvalidToken", "."));
        assert!(!is_ignored_key(
            &ignored,
            "Server_ErrorsExtra.InvalidToken",
            "."
        ));
        assert!(is_ignored_key(
            &ignored,
            "Backend//Validation//Required",
            "//"
        ));
        assert!(!is_ignored_key(&ignored, "Backend//Validator", "//"));
    }

    #[test]
    fn workspace_partitions_ignored_keys_out_of_normal_indexes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"],
              "ignoredScopes":["Server_Errors"]
            }"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("translation.en.json"),
            r#"{"Page":{"title":"Title"},"Server_Errors":{"Invalid":"Invalid"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("app.ts"),
            concat!(
                "i18next.t('Page.title');",
                "i18next.t('Server_Errors.Invalid');",
                "const i18n=useScopedTranslation('Server_Errors');",
                "i18n.t('Invalid');"
            ),
        )
        .unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let snapshot = workspace.snapshot();
        let ignored_key = CanonicalKey::new("Server_Errors.Invalid", ".").unwrap();
        let normal_key = CanonicalKey::new("Page.title", ".").unwrap();
        assert!(snapshot.dictionary_entries(&ignored_key).is_empty());
        assert!(snapshot.occurrences(&ignored_key).is_empty());
        assert_eq!(snapshot.occurrences(&normal_key).len(), 1);
        assert_eq!(snapshot.ignored_occurrences().count(), 2);
    }
    #[cfg(windows)]
    #[test]
    fn file_lookup_ignores_windows_uri_casing() {
        let indexed_uri = Url::parse("file:///C:/Project/App.ts").unwrap();
        let requested_uri = Url::parse("file:///c%3A/project/app.ts").unwrap();
        let contribution = FileContribution {
            uri: indexed_uri.clone(),
            text: "const value = 1;".into(),
            version: None,
            dictionaries: vec![],
            occurrences: vec![],
            ignored_occurrences: vec![],
            bindings: vec![],
        };
        let snapshot = IndexSnapshot::rebuild(
            1,
            HashMap::from([(indexed_uri.clone(), Arc::new(contribution))]),
        );
        assert_eq!(snapshot.text(&requested_uri), Some("const value = 1;"));
        assert_eq!(normalized_uri(&indexed_uri), normalized_uri(&requested_uri));
    }

    #[test]
    fn searches_relative_keys_but_not_values() {
        let uri = Url::parse("file:///translation.en.json").unwrap();
        let text = r#"{"Page":{"Login":{"my_key":"My value"}}}"#.to_string();
        let dictionaries = parse_dictionary(&uri, "en", &text, ".").unwrap();
        let contribution = FileContribution {
            uri: uri.clone(),
            text,
            version: None,
            dictionaries,
            occurrences: vec![],
            ignored_occurrences: vec![],
            bindings: vec![],
        };
        let snapshot = IndexSnapshot::rebuild(1, HashMap::from([(uri, Arc::new(contribution))]));
        let scope = CanonicalKey::new("Page.Login", ".").unwrap();
        let (found, _) = snapshot.completions(
            &CompletionContext::ScopedKey {
                scope: scope.clone(),
                query: "my_k".into(),
            },
            "en",
            ".",
            20,
        );
        assert_eq!(found[0].key, "my_key");

        let (found, _) = snapshot.completions(
            &CompletionContext::ScopedKey {
                scope,
                query: "My value".into(),
            },
            "en",
            ".",
            20,
        );
        assert!(found.is_empty());
    }

    #[test]
    fn reports_only_unreferenced_default_locale_leaves_as_unused() {
        let dictionary_uri = Url::parse("file:///translation.en.json").unwrap();
        let dictionary_text = r#"{"used":"Used","unused":"Unused"}"#.to_string();
        let dictionaries = parse_dictionary(&dictionary_uri, "en", &dictionary_text, ".").unwrap();
        let dictionary = FileContribution {
            uri: dictionary_uri.clone(),
            text: dictionary_text,
            version: None,
            dictionaries,
            occurrences: vec![],
            ignored_occurrences: vec![],
            bindings: vec![],
        };

        let source_uri = Url::parse("file:///app.ts").unwrap();
        let source_text = "i18next.t('used')".to_string();
        let (occurrences, bindings) = analyze_source(
            &source_uri,
            &source_text,
            ".",
            &["useScopedTranslation".into()],
            &["t".into()],
            &["i18next.t".into()],
            &[],
            &[],
        );
        let source = FileContribution {
            uri: source_uri.clone(),
            text: source_text,
            version: None,
            dictionaries: vec![],
            occurrences,
            ignored_occurrences: vec![],
            bindings,
        };
        let snapshot = IndexSnapshot::rebuild(
            1,
            HashMap::from([
                (dictionary_uri, Arc::new(dictionary)),
                (source_uri, Arc::new(source)),
            ]),
        );

        let leaves = snapshot
            .default_locale_leaf_entries("en")
            .map(|entry| (entry.key.as_str(), snapshot.is_leaf_key_used(&entry.key)))
            .collect::<Vec<_>>();
        assert_eq!(leaves, vec![("unused", false), ("used", true)]);
    }

    #[test]
    fn scope_references_are_recursive_only_for_small_scopes() {
        fn snapshot_with_leaf_count(count: usize) -> IndexSnapshot {
            let dictionary_uri = Url::parse("file:///translation.en.json").unwrap();
            let children = (0..count)
                .map(|index| format!(r#""Child{index}":{{"leaf":"value"}}"#))
                .collect::<Vec<_>>()
                .join(",");
            let dictionary_text = format!(r#"{{"Scope":{{{children}}}}}"#);
            let dictionaries =
                parse_dictionary(&dictionary_uri, "en", &dictionary_text, ".").unwrap();
            let dictionary = FileContribution {
                uri: dictionary_uri.clone(),
                text: dictionary_text,
                version: None,
                dictionaries,
                occurrences: vec![],
                ignored_occurrences: vec![],
                bindings: vec![],
            };

            let source_uri = Url::parse("file:///app.ts").unwrap();
            let source_text = concat!(
                "i18next.t('Scope.Child0.leaf');",
                "useScopedTranslation('Scope.Child0')"
            )
            .to_string();
            let (occurrences, bindings) = analyze_source(
                &source_uri,
                &source_text,
                ".",
                &["useScopedTranslation".into()],
                &["t".into()],
                &["i18next.t".into()],
                &[],
                &[],
            );
            let source = FileContribution {
                uri: source_uri.clone(),
                text: source_text,
                version: None,
                dictionaries: vec![],
                occurrences,
                ignored_occurrences: vec![],
                bindings,
            };
            IndexSnapshot::rebuild(
                1,
                HashMap::from([
                    (dictionary_uri, Arc::new(dictionary)),
                    (source_uri, Arc::new(source)),
                ]),
            )
        }

        let scope = CanonicalKey::new("Scope", ".").unwrap();
        let small = snapshot_with_leaf_count(2);
        assert_eq!(small.scope_occurrences(&scope, ".", 32).len(), 2);

        let large = snapshot_with_leaf_count(33);
        let occurrences = large.scope_occurrences(&scope, ".", 32);
        assert_eq!(occurrences.len(), 1);
        assert_eq!(occurrences[0].kind, OccurrenceKind::ScopeDeclaration);
        assert_eq!(occurrences[0].key.as_str(), "Scope.Child0");
    }

    #[test]
    fn dynamic_scope_marks_every_descendant_used_and_referenced() {
        let dictionary_uri = Url::parse("file:///translation.en.json").unwrap();
        let dictionary_text = r#"{"SomeScope":{"child":{"leaf":"Value"}}}"#.to_string();
        let dictionary = FileContribution {
            uri: dictionary_uri.clone(),
            dictionaries: parse_dictionary(&dictionary_uri, "en", &dictionary_text, ".").unwrap(),
            text: dictionary_text,
            version: None,
            occurrences: vec![],
            ignored_occurrences: vec![],
            bindings: vec![],
        };
        let source_uri = Url::parse("file:///app.ts").unwrap();
        let source_text = "i18next.t(`SomeScope.${value}`)".to_string();
        let (occurrences, bindings) = analyze_source(
            &source_uri,
            &source_text,
            ".",
            &["useScopedTranslation".into()],
            &["t".into()],
            &["i18next.t".into()],
            &[],
            &[],
        );
        let source = FileContribution {
            uri: source_uri,
            text: source_text,
            version: None,
            dictionaries: vec![],
            occurrences,
            ignored_occurrences: vec![],
            bindings,
        };
        let snapshot = IndexSnapshot::rebuild(
            1,
            HashMap::from([
                (dictionary_uri, Arc::new(dictionary)),
                (source.uri.clone(), Arc::new(source)),
            ]),
        );
        let leaf = CanonicalKey::new("SomeScope.child.leaf", ".").unwrap();
        assert!(snapshot.is_leaf_key_used_with_separator(&leaf, "."));
        let references = snapshot
            .dynamic_scope_occurrences(&leaf, ".")
            .collect::<Vec<_>>();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].key.as_str(), "SomeScope");
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
            &[],
            &[],
        );
        let contribution = FileContribution {
            uri: uri.clone(),
            text: text.clone(),
            version: None,
            dictionaries: vec![],
            occurrences,
            ignored_occurrences: vec![],
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

    #[test]
    fn reports_malformed_dictionary_during_workspace_load() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{"dictionaries":"translation.{locale}.json","defaultLocale":"en","scopedFunctions":["useScopedTranslation"],"translationMethods":["t"],"fullKeyFunctions":["i18next.t"]}"#,
        )
        .unwrap();
        std::fs::write(temp.path().join("translation.en.json"), "{ invalid").unwrap();

        let result = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        );
        assert!(matches!(
            result,
            Err(crate::ConfigError::InvalidDictionary { .. })
        ));
    }

    #[test]
    fn recovers_after_an_invalid_intermediate_dictionary_edit() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "keySeparator":".",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"]
            }"#,
        )
        .unwrap();
        let dictionary_path = temp.path().join("translation.en.json");
        std::fs::write(&dictionary_path, r#"{"Page":{"old":"Old"}}"#).unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let uri = Url::from_file_path(dictionary_path).unwrap();

        workspace.update_text(uri.clone(), r#"{"Page":{"new":}}"#.into(), Some(1));
        assert_eq!(
            workspace.snapshot().text(&uri),
            Some(r#"{"Page":{"new":}}"#)
        );

        workspace.update_text(uri, r#"{"Page":{"new":"New"}}"#.into(), Some(2));
        let key = CanonicalKey::new("Page.new", ".").unwrap();
        assert_eq!(workspace.snapshot().dictionary_entries(&key).len(), 1);
    }

    #[test]
    fn external_disk_change_refreshes_an_open_document_and_resets_its_version() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "keySeparator":".",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"]
            }"#,
        )
        .unwrap();
        let dictionary_path = temp.path().join("translation.en.json");
        std::fs::write(&dictionary_path, r#"{"Page":{"key":"Value"}}"#).unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let uri = Url::from_file_path(&dictionary_path).unwrap();

        workspace.update_text(uri.clone(), r#"{"Page":{"key":"Editor"}}"#.into(), Some(10));
        let disk_text = "{\n  \"Page\": {\n    \"key\": \"External\"\n  }\n}";
        std::fs::write(&dictionary_path, disk_text).unwrap();
        let _ = workspace.refresh_disk_path(&dictionary_path);
        assert_eq!(workspace.snapshot().text(&uri), Some(disk_text));

        // An editor may restart its document version after reloading an
        // external change. The disk refresh must let that lower version win.
        workspace.update_text(
            uri.clone(),
            r#"{"Page":{"key":"Reloaded"}}"#.into(),
            Some(1),
        );
        assert_eq!(
            workspace.snapshot().text(&uri),
            Some(r#"{"Page":{"key":"Reloaded"}}"#)
        );
    }
}
