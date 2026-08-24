use crate::{ByteRange, CanonicalKey};
use tree_sitter::{Node, Parser};
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    Object,
    Leaf,
}

#[derive(Clone, Debug)]
pub struct DictionaryEntry {
    pub uri: Url,
    pub key: CanonicalKey,
    pub locale: String,
    pub value: Option<String>,
    pub key_range: ByteRange,
    pub value_range: ByteRange,
    pub kind: EntryKind,
}

#[derive(Debug, thiserror::Error)]
pub enum DictionaryError {
    #[error("dictionary is not valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("could not initialize the JSON parser")]
    Parser,
}

pub fn parse_dictionary(
    uri: &Url,
    locale: &str,
    text: &str,
    separator: &str,
) -> Result<Vec<DictionaryEntry>, DictionaryError> {
    let _: serde_json::Value = serde_json::from_str(text)?;
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_json::LANGUAGE.into())
        .map_err(|_| DictionaryError::Parser)?;
    let tree = parser.parse(text, None).ok_or(DictionaryError::Parser)?;
    let mut entries = Vec::new();
    let root = tree.root_node();
    let value = root.named_child(0).unwrap_or(root);
    visit_value(
        uri,
        locale,
        text,
        separator,
        value,
        &mut Vec::new(),
        &mut entries,
    );
    Ok(entries)
}

fn visit_value(
    uri: &Url,
    locale: &str,
    text: &str,
    separator: &str,
    node: Node<'_>,
    path: &mut Vec<String>,
    out: &mut Vec<DictionaryEntry>,
) {
    if node.kind() != "object" {
        return;
    }
    let mut cursor = node.walk();
    for pair in node
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "pair")
    {
        let Some(key_node) = pair.child_by_field_name("key") else {
            continue;
        };
        let Some(value_node) = pair.child_by_field_name("value") else {
            continue;
        };
        let Ok(raw_key) = key_node.utf8_text(text.as_bytes()) else {
            continue;
        };
        let Ok(segment) = serde_json::from_str::<String>(raw_key) else {
            continue;
        };
        path.push(segment);
        let joined = path.join(separator);
        if let Some(key) = CanonicalKey::new(joined, separator) {
            let key_range = string_content_range(key_node);
            let kind = if value_node.kind() == "object" {
                EntryKind::Object
            } else {
                EntryKind::Leaf
            };
            let value = if value_node.kind() == "string" {
                value_node
                    .utf8_text(text.as_bytes())
                    .ok()
                    .and_then(|s| serde_json::from_str(s).ok())
            } else {
                None
            };
            if kind == EntryKind::Object || value.is_some() {
                out.push(DictionaryEntry {
                    uri: uri.clone(),
                    key,
                    locale: locale.to_owned(),
                    value,
                    key_range,
                    value_range: ByteRange(value_node.byte_range()),
                    kind,
                });
            }
        }
        visit_value(uri, locale, text, separator, value_node, path, out);
        path.pop();
    }
}

fn string_content_range(node: Node<'_>) -> ByteRange {
    let range = node.byte_range();
    ByteRange((range.start + 1)..range.end.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flattens_and_retains_ranges() {
        let text = r#"{"Page":{"Login":{"submit":"Sign in"}}}"#;
        let uri = Url::parse("file:///translation.en.json").unwrap();
        let entries = parse_dictionary(&uri, "en", text, ".").unwrap();
        assert_eq!(entries.len(), 3);
        let leaf = entries.last().unwrap();
        assert_eq!(leaf.key.as_str(), "Page.Login.submit");
        assert_eq!(&text[leaf.key_range.0.clone()], "submit");
        assert_eq!(leaf.value.as_deref(), Some("Sign in"));
    }
}
