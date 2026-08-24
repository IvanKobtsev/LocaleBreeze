use crate::{ByteRange, CanonicalKey};
use tree_sitter::{Node, Parser};
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OccurrenceKind {
    FullKey,
    ScopedKey,
    ScopeDeclaration,
}

#[derive(Clone, Debug)]
pub struct SourceOccurrence {
    pub uri: Url,
    pub range: ByteRange,
    pub key: CanonicalKey,
    pub kind: OccurrenceKind,
    pub scope: Option<CanonicalKey>,
    pub relative_key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ScopeBinding {
    pub name: String,
    pub method: String,
    pub scope: CanonicalKey,
    pub declaration_range: ByteRange,
    pub visibility: ByteRange,
    pub direct_function: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn analyze_source(
    uri: &Url,
    text: &str,
    separator: &str,
    scoped_functions: &[String],
    methods: &[String],
    full_key_functions: &[String],
    translation_key_types: &[String],
    translation_key_props: &[String],
) -> (Vec<SourceOccurrence>, Vec<ScopeBinding>) {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .is_err()
    {
        return (vec![], vec![]);
    }
    let Some(tree) = parser.parse(text, None) else {
        return (vec![], vec![]);
    };
    let root = tree.root_node();
    let mut bindings = Vec::new();
    collect_bindings(
        root,
        text,
        separator,
        scoped_functions,
        methods,
        &mut bindings,
    );
    bindings.sort_by_key(|b| b.declaration_range.0.start);
    let mut occurrences = Vec::new();
    collect_calls(
        root,
        uri,
        text,
        separator,
        scoped_functions,
        full_key_functions,
        &bindings,
        &mut occurrences,
    );
    collect_lexical_key_sinks(
        root,
        uri,
        text,
        separator,
        translation_key_types,
        translation_key_props,
        &mut occurrences,
    );
    (occurrences, bindings)
}

#[allow(clippy::collapsible_if)]
fn collect_bindings(
    node: Node<'_>,
    text: &str,
    separator: &str,
    scoped_functions: &[String],
    methods: &[String],
    out: &mut Vec<ScopeBinding>,
) {
    if node.kind() == "variable_declarator" {
        if let (Some(name), Some(value)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("value"),
        ) {
            if let Some((callee, literal)) = call_with_literal(value, text) {
                if scoped_functions.iter().any(|x| x == &callee) {
                    if let Some(scope) =
                        literal_value(literal, text).and_then(|s| CanonicalKey::new(s, separator))
                    {
                        let visibility_end = lexical_container(node).end_byte();
                        let visibility = ByteRange(node.end_byte()..visibility_end);
                        let declaration_range = content_range(literal);
                        if name.kind() == "identifier" {
                            if let Ok(binding_name) = name.utf8_text(text.as_bytes()) {
                                for method in methods {
                                    out.push(ScopeBinding {
                                        name: binding_name.into(),
                                        method: method.clone(),
                                        scope: scope.clone(),
                                        declaration_range: declaration_range.clone(),
                                        visibility: visibility.clone(),
                                        direct_function: false,
                                    });
                                }
                            }
                        } else if name.kind() == "object_pattern" {
                            let mut cursor = name.walk();
                            for child in name.named_children(&mut cursor) {
                                let (property, local) = match child.kind() {
                                    "shorthand_property_identifier_pattern" => {
                                        let n = child.utf8_text(text.as_bytes()).unwrap_or("");
                                        (n, n)
                                    }
                                    "pair_pattern" => {
                                        let key = child
                                            .child_by_field_name("key")
                                            .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                                            .unwrap_or("");
                                        let val = child
                                            .child_by_field_name("value")
                                            .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                                            .unwrap_or("");
                                        (key, val)
                                    }
                                    _ => continue,
                                };
                                if methods.iter().any(|m| m == property) {
                                    out.push(ScopeBinding {
                                        name: local.into(),
                                        method: property.into(),
                                        scope: scope.clone(),
                                        declaration_range: declaration_range.clone(),
                                        visibility: visibility.clone(),
                                        direct_function: true,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_bindings(child, text, separator, scoped_functions, methods, out);
    }
}

#[allow(clippy::collapsible_if, clippy::too_many_arguments)]
fn collect_calls(
    node: Node<'_>,
    uri: &Url,
    text: &str,
    separator: &str,
    scoped_functions: &[String],
    full_key_functions: &[String],
    bindings: &[ScopeBinding],
    out: &mut Vec<SourceOccurrence>,
) {
    if node.kind() == "call_expression" {
        if let Some((callee, literal)) = call_with_literal(node, text) {
            if let Some(value) = literal_value(literal, text) {
                let range = content_range(literal);
                if scoped_functions.iter().any(|x| x == &callee) {
                    if let Some(key) = CanonicalKey::new(value, separator) {
                        out.push(SourceOccurrence {
                            uri: uri.clone(),
                            range,
                            key,
                            kind: OccurrenceKind::ScopeDeclaration,
                            scope: None,
                            relative_key: None,
                        });
                    }
                } else if full_key_functions.iter().any(|x| x == &callee) {
                    if let Some(key) = CanonicalKey::new(value, separator) {
                        out.push(SourceOccurrence {
                            uri: uri.clone(),
                            range,
                            key,
                            kind: OccurrenceKind::FullKey,
                            scope: None,
                            relative_key: None,
                        });
                    }
                } else if let Some(binding) = resolve_binding(&callee, node.start_byte(), bindings)
                {
                    if let Some(key) = CanonicalKey::join(&binding.scope, &value, separator) {
                        out.push(SourceOccurrence {
                            uri: uri.clone(),
                            range,
                            key,
                            kind: OccurrenceKind::ScopedKey,
                            scope: Some(binding.scope.clone()),
                            relative_key: Some(value),
                        });
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_calls(
            child,
            uri,
            text,
            separator,
            scoped_functions,
            full_key_functions,
            bindings,
            out,
        );
    }
}

fn resolve_binding<'a>(
    callee: &str,
    at: usize,
    bindings: &'a [ScopeBinding],
) -> Option<&'a ScopeBinding> {
    bindings.iter().rev().find(|b| {
        b.visibility.contains(at)
            && if b.direct_function {
                callee == b.name
            } else {
                callee == format!("{}.{}", b.name, b.method)
            }
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_lexical_key_sinks(
    node: Node<'_>,
    uri: &Url,
    text: &str,
    separator: &str,
    configured_types: &[String],
    configured_props: &[String],
    out: &mut Vec<SourceOccurrence>,
) {
    let literal = match node.kind() {
        "variable_declarator" => node
            .child_by_field_name("type")
            .filter(|ty| contains_configured_type(*ty, text, configured_types))
            .and_then(|_| node.child_by_field_name("value"))
            .and_then(string_literal_in),
        "as_expression" | "satisfies_expression" | "type_assertion" => node
            .child_by_field_name("type")
            .or_else(|| node.named_child(node.named_child_count().saturating_sub(1)))
            .filter(|ty| contains_configured_type(*ty, text, configured_types))
            .and_then(|_| {
                node.child_by_field_name("expression")
                    .or_else(|| node.named_child(0))
            })
            .and_then(string_literal_in),
        "jsx_attribute" => node
            .child_by_field_name("name")
            .or_else(|| node.named_child(0))
            .and_then(|name| name.utf8_text(text.as_bytes()).ok())
            .filter(|name| configured_props.iter().any(|prop| prop == name))
            .and_then(|_| {
                node.child_by_field_name("value")
                    .or_else(|| node.named_child(1))
            })
            .and_then(string_literal_in),
        "pair" => node
            .child_by_field_name("key")
            .and_then(|key| property_name(key, text))
            .filter(|name| configured_props.iter().any(|prop| prop == name))
            .and_then(|_| node.child_by_field_name("value"))
            .and_then(string_literal_in),
        _ => None,
    };

    if let Some(literal) = literal
        && let Some(value) = literal_value(literal, text)
        && let Some(key) = CanonicalKey::new(value, separator)
    {
        out.push(SourceOccurrence {
            uri: uri.clone(),
            range: content_range(literal),
            key,
            kind: OccurrenceKind::FullKey,
            scope: None,
            relative_key: None,
        });
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_lexical_key_sinks(
            child,
            uri,
            text,
            separator,
            configured_types,
            configured_props,
            out,
        );
    }
}

fn contains_configured_type(node: Node<'_>, text: &str, configured: &[String]) -> bool {
    if matches!(
        node.kind(),
        "type_identifier" | "nested_type_identifier" | "identifier"
    ) && node.utf8_text(text.as_bytes()).ok().is_some_and(|name| {
        configured
            .iter()
            .any(|configured| configured == name || name.ends_with(&format!(".{configured}")))
    }) {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| contains_configured_type(child, text, configured))
}

fn string_literal_in(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "string" {
        return Some(node);
    }
    if matches!(node.kind(), "jsx_expression" | "parenthesized_expression") {
        let mut cursor = node.walk();
        return node.named_children(&mut cursor).find_map(string_literal_in);
    }
    None
}

fn property_name<'a>(node: Node<'a>, text: &'a str) -> Option<&'a str> {
    let raw = node.utf8_text(text.as_bytes()).ok()?;
    if node.kind() == "string" {
        raw.get(1..raw.len().saturating_sub(1))
    } else {
        Some(raw)
    }
}

fn call_with_literal<'a>(node: Node<'a>, text: &str) -> Option<(String, Node<'a>)> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    let callee = function.utf8_text(text.as_bytes()).ok()?.to_owned();
    let args = node.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let literal = args.named_children(&mut cursor).next()?;
    matches!(literal.kind(), "string" | "string_fragment").then_some((callee, literal))
}

fn literal_value(node: Node<'_>, text: &str) -> Option<String> {
    let raw = node.utf8_text(text.as_bytes()).ok()?;
    if raw.len() < 2 {
        return None;
    }
    let quote = raw.as_bytes()[0];
    if !matches!(quote, b'\'' | b'\"') || raw.as_bytes()[raw.len() - 1] != quote {
        return None;
    }
    if quote == b'\"' {
        serde_json::from_str(raw).ok()
    } else {
        Some(
            raw[1..raw.len() - 1]
                .replace("\\'", "'")
                .replace("\\\\", "\\"),
        )
    }
}

fn content_range(node: Node<'_>) -> ByteRange {
    ByteRange((node.start_byte() + 1)..node.end_byte().saturating_sub(1))
}

fn lexical_container(mut node: Node<'_>) -> Node<'_> {
    while let Some(parent) = node.parent() {
        node = parent;
        if matches!(node.kind(), "statement_block" | "program") {
            return node;
        }
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_supported_patterns() {
        let text = r#"
          const i18n = useScopedTranslation('Page.Login');
          i18n.t('submit');
          i18n.key('page_title');
          const { t: tr } = useScopedTranslation("Page.Home");
          tr('title');
          i18next.t('Page.Login.cancel');
        "#;
        let uri = Url::parse("file:///app.tsx").unwrap();
        let (found, bindings) = analyze_source(
            &uri,
            text,
            ".",
            &["useScopedTranslation".into()],
            &["t".into(), "key".into()],
            &["i18next.t".into()],
            &[],
            &[],
        );
        assert_eq!(bindings.len(), 3);
        let keys: Vec<_> = found
            .iter()
            .filter(|o| o.kind != OccurrenceKind::ScopeDeclaration)
            .map(|o| o.key.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "Page.Login.submit",
                "Page.Login.page_title",
                "Page.Home.title",
                "Page.Login.cancel"
            ]
        );
    }

    #[test]
    fn ignores_dynamic_calls() {
        let uri = Url::parse("file:///app.ts").unwrap();
        let (found, _) = analyze_source(
            &uri,
            "const x=useScopedTranslation(getScope()); x.t(key)",
            ".",
            &["useScopedTranslation".into()],
            &["t".into()],
            &["i18next.t".into()],
            &[],
            &[],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn recognizes_configured_lexical_key_sinks() {
        let text = r#"
          const typed: TranslationKey = 'Page.Typed';
          const asserted = 'Page.Asserted' as TranslationKey;
          const satisfied = 'Page.Satisfied' satisfies TranslationKey;
          const ignored: string = 'Page.Ignored';
          const jsx = <><Card transKey="Page.Jsx"/><Card transKey={'Page.Expression'}/></>;
          const object = { transKey: 'Page.Object', other: 'Page.Other' };
        "#;
        let uri = Url::parse("file:///app.tsx").unwrap();
        let (found, _) = analyze_source(
            &uri,
            text,
            ".",
            &["useScopedTranslation".into()],
            &["t".into()],
            &["i18next.t".into()],
            &["TranslationKey".into()],
            &["transKey".into()],
        );
        let keys = found
            .iter()
            .map(|occurrence| occurrence.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "Page.Typed",
                "Page.Asserted",
                "Page.Satisfied",
                "Page.Jsx",
                "Page.Expression",
                "Page.Object"
            ]
        );
    }
}
