mod config;
mod dictionary;
mod index;
mod line_index;
mod source;

pub use config::{Config, ConfigError, DictionaryPattern};
pub use dictionary::{DictionaryEntry, DictionaryError, EntryKind, parse_dictionary};
pub use index::{CompletionCandidate, CompletionContext, IndexSnapshot, WorkspaceIndex};
pub use line_index::LineIndex;
pub use source::{OccurrenceKind, ScopeBinding, SourceOccurrence, analyze_source};

use std::fmt;
use std::ops::Range;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanonicalKey(String);

impl CanonicalKey {
    pub fn new(value: impl Into<String>, separator: &str) -> Option<Self> {
        let value = value.into();
        if value.is_empty() || separator.is_empty() || value.split(separator).any(str::is_empty) {
            return None;
        }
        Some(Self(value))
    }

    pub fn join(scope: &Self, relative: &str, separator: &str) -> Option<Self> {
        Self::new(format!("{}{}{}", scope.0, separator, relative), separator)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parent(&self, separator: &str) -> Option<Self> {
        self.0
            .rsplit_once(separator)
            .and_then(|(p, _)| Self::new(p, separator))
    }

    pub fn relative_to(&self, scope: &Self, separator: &str) -> Option<&str> {
        self.0.strip_prefix(scope.as_str())?.strip_prefix(separator)
    }
}

impl fmt::Display for CanonicalKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByteRange(pub Range<usize>);

impl ByteRange {
    pub fn contains(&self, offset: usize) -> bool {
        self.0.start <= offset && offset <= self.0.end
    }
}

#[derive(Clone, Debug)]
pub struct FileContribution {
    pub uri: url::Url,
    pub text: String,
    pub version: Option<i32>,
    pub dictionaries: Vec<DictionaryEntry>,
    pub occurrences: Vec<SourceOccurrence>,
    pub bindings: Vec<ScopeBinding>,
}
