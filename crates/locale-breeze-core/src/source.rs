use crate::{ByteRange, CanonicalKey, FullKeyFunctionConfig, ScopedFunctionConfig};
use tree_sitter::{Node, Parser};
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OccurrenceKind {
    FullKey,
    ScopedKey,
    ScopeDeclaration,
    DynamicScope,
    NamespaceDeclaration,
}

#[derive(Clone, Debug)]
pub struct SourceOccurrence {
    pub uri: Url,
    pub range: ByteRange,
    pub key: CanonicalKey,
    pub namespace: Option<String>,
    pub kind: OccurrenceKind,
    pub scope: Option<CanonicalKey>,
    pub relative_key: Option<String>,
    pub arguments: Option<TranslationArguments>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranslationArgument {
    pub name: String,
    pub range: ByteRange,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TranslationArguments {
    pub supplied: Vec<TranslationArgument>,
    pub uncertain: bool,
}

impl TranslationArguments {
    pub fn has(&self, name: &str) -> bool {
        self.supplied.iter().any(|argument| argument.name == name)
    }
}

#[derive(Clone, Debug)]
pub struct ScopeBinding {
    pub name: String,
    pub method: String,
    pub scope: Option<CanonicalKey>,
    pub namespace: Option<String>,
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
    let scoped_functions = scoped_functions
        .iter()
        .map(|function_name| ScopedFunctionConfig {
            function_name: function_name.clone(),
            default_namespace: None,
            translation_methods: methods.to_vec(),
        })
        .collect::<Vec<_>>();
    let full_key_functions = full_key_functions
        .iter()
        .map(|function_name| FullKeyFunctionConfig {
            function_name: function_name.clone(),
            default_namespace: None,
        })
        .collect::<Vec<_>>();
    analyze_source_with_namespace(
        uri,
        text,
        separator,
        &scoped_functions,
        &full_key_functions,
        translation_key_types,
        translation_key_props,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn analyze_source_with_namespace(
    uri: &Url,
    text: &str,
    separator: &str,
    scoped_functions: &[ScopedFunctionConfig],
    full_key_functions: &[FullKeyFunctionConfig],
    translation_key_types: &[String],
    translation_key_props: &[String],
    default_namespace: Option<&str>,
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
        default_namespace,
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
        default_namespace,
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
        default_namespace,
        &mut occurrences,
    );
    (occurrences, bindings)
}

#[allow(clippy::collapsible_if)]
fn collect_bindings(
    node: Node<'_>,
    text: &str,
    separator: &str,
    scoped_functions: &[ScopedFunctionConfig],
    default_namespace: Option<&str>,
    out: &mut Vec<ScopeBinding>,
) {
    if node.kind() == "variable_declarator" {
        if let (Some(name), Some(value)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("value"),
        ) {
            if let Some((callee, literal)) = call_with_literal(value, text) {
                if let Some(function) = scoped_functions
                    .iter()
                    .find(|function| function.function_name == callee)
                {
                    if let Some(scope) =
                        literal_value(literal, text).and_then(|s| CanonicalKey::new(s, separator))
                    {
                        let visibility_end = lexical_container(node).end_byte();
                        let visibility = ByteRange(node.end_byte()..visibility_end);
                        let declaration_range = content_range(literal);
                        if name.kind() == "identifier" {
                            if let Ok(binding_name) = name.utf8_text(text.as_bytes()) {
                                for method in &function.translation_methods {
                                    out.push(ScopeBinding {
                                        name: binding_name.into(),
                                        method: method.clone(),
                                        scope: Some(scope.clone()),
                                        namespace: function
                                            .default_namespace
                                            .as_deref()
                                            .or(default_namespace)
                                            .map(str::to_owned),
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
                                if function
                                    .translation_methods
                                    .iter()
                                    .any(|method| method == property)
                                {
                                    out.push(ScopeBinding {
                                        name: local.into(),
                                        method: property.into(),
                                        scope: Some(scope.clone()),
                                        namespace: function
                                            .default_namespace
                                            .as_deref()
                                            .or(default_namespace)
                                            .map(str::to_owned),
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
            if is_call_named(value, text, "useTranslation") {
                let namespace = match call_argument(value, 0) {
                    None => default_namespace.map(str::to_owned),
                    Some(argument)
                        if argument.utf8_text(text.as_bytes()).ok() == Some("undefined") =>
                    {
                        default_namespace.map(str::to_owned)
                    }
                    Some(argument) => literal_value(argument, text),
                };
                if let Some(namespace) = namespace {
                    let scope = call_argument(value, 1)
                        .and_then(|options| object_string_property(options, text, "keyPrefix"))
                        .and_then(|value| CanonicalKey::new(value, separator));
                    let visibility = ByteRange(node.end_byte()..lexical_container(node).end_byte());
                    let declaration_range = call_argument(value, 0)
                        .filter(|arg| literal_value(*arg, text).is_some())
                        .map(content_range)
                        .unwrap_or_else(|| ByteRange(value.start_byte()..value.start_byte()));
                    push_bindings(
                        name,
                        text,
                        &["t".into()],
                        namespace,
                        scope,
                        declaration_range,
                        visibility,
                        out,
                    );
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_bindings(
            child,
            text,
            separator,
            scoped_functions,
            default_namespace,
            out,
        );
    }
}

#[allow(clippy::collapsible_if, clippy::too_many_arguments)]
fn collect_calls(
    node: Node<'_>,
    uri: &Url,
    text: &str,
    separator: &str,
    scoped_functions: &[ScopedFunctionConfig],
    full_key_functions: &[FullKeyFunctionConfig],
    default_namespace: Option<&str>,
    bindings: &[ScopeBinding],
    out: &mut Vec<SourceOccurrence>,
) {
    if node.kind() == "call_expression" {
        if is_call_named(node, text, "useTranslation")
            && let Some(prefix_node) = call_argument(node, 1)
                .and_then(|options| object_string_property_node(options, text, "keyPrefix"))
            && let Some(prefix) = literal_value(prefix_node, text)
            && let Some(key) = CanonicalKey::new(prefix, separator)
        {
            let namespace = match call_argument(node, 0) {
                None => default_namespace.map(str::to_owned),
                Some(argument) if argument.utf8_text(text.as_bytes()).ok() == Some("undefined") => {
                    default_namespace.map(str::to_owned)
                }
                Some(argument) => literal_value(argument, text),
            };
            if namespace.is_some() || default_namespace.is_none() {
                out.push(SourceOccurrence {
                    uri: uri.clone(),
                    range: content_range(prefix_node),
                    key,
                    namespace,
                    kind: OccurrenceKind::ScopeDeclaration,
                    scope: None,
                    relative_key: None,
                    arguments: None,
                });
            }
        }
        if let Some((callee, argument)) = call_with_first_argument(node, text) {
            if let Some(value) = literal_value(argument, text) {
                let literal = argument;
                let range = content_range(literal);
                if let Some(function) = scoped_functions
                    .iter()
                    .find(|function| function.function_name == callee)
                {
                    if let Some(key) = CanonicalKey::new(value, separator) {
                        out.push(SourceOccurrence {
                            uri: uri.clone(),
                            range,
                            key,
                            namespace: function
                                .default_namespace
                                .as_deref()
                                .or(default_namespace)
                                .map(str::to_owned),
                            kind: OccurrenceKind::ScopeDeclaration,
                            scope: None,
                            relative_key: None,
                            arguments: None,
                        });
                    }
                } else if callee == "useTranslation" {
                    if let Some(key) = CanonicalKey::new(&value, separator) {
                        out.push(SourceOccurrence {
                            uri: uri.clone(),
                            range,
                            key,
                            namespace: Some(value),
                            kind: OccurrenceKind::NamespaceDeclaration,
                            scope: None,
                            relative_key: None,
                            arguments: None,
                        });
                    }
                } else if callee == "i18next.t"
                    || full_key_functions
                        .iter()
                        .any(|function| function.function_name == callee)
                {
                    if let Some(initial_key) = CanonicalKey::new(&value, separator) {
                        let (namespace, key) = split_namespace_key(&value, separator).map_or(
                            (
                                (callee == "i18next.t")
                                    .then(|| call_namespace_option(node, text))
                                    .flatten()
                                    .or_else(|| {
                                        full_key_functions
                                            .iter()
                                            .find(|function| function.function_name == callee)
                                            .and_then(|function| function.default_namespace.clone())
                                            .or_else(|| default_namespace.map(str::to_owned))
                                    }),
                                initial_key,
                            ),
                            |(namespace, key)| (Some(namespace), key),
                        );
                        out.push(SourceOccurrence {
                            uri: uri.clone(),
                            range,
                            key,
                            namespace,
                            kind: OccurrenceKind::FullKey,
                            scope: None,
                            relative_key: None,
                            arguments: Some(translation_arguments(node, text)),
                        });
                    }
                } else if let Some(binding) = resolve_binding(&callee, node.start_byte(), bindings)
                {
                    let explicit = split_namespace_key(&value, separator);
                    let namespace = explicit
                        .as_ref()
                        .map(|x| x.0.clone())
                        .or_else(|| binding.namespace.clone());
                    let relative = explicit.as_ref().map_or(value.as_str(), |x| x.1.as_str());
                    let key = binding
                        .scope
                        .as_ref()
                        .and_then(|scope| CanonicalKey::join(scope, relative, separator))
                        .or_else(|| CanonicalKey::new(relative, separator));
                    if let Some(key) = key {
                        out.push(SourceOccurrence {
                            uri: uri.clone(),
                            range,
                            key,
                            namespace,
                            kind: OccurrenceKind::ScopedKey,
                            scope: binding.scope.clone(),
                            relative_key: Some(relative.to_owned()),
                            arguments: Some(translation_arguments(node, text)),
                        });
                    }
                }
            } else if let Some((prefix, range)) = dynamic_template_prefix(argument, text, separator)
            {
                let (key, scope, relative_key) = if callee == "i18next.t"
                    || full_key_functions
                        .iter()
                        .any(|function| function.function_name == callee)
                {
                    (CanonicalKey::new(&prefix, separator), None, None)
                } else if let Some(binding) = resolve_binding(&callee, node.start_byte(), bindings)
                {
                    (
                        binding
                            .scope
                            .as_ref()
                            .and_then(|scope| CanonicalKey::join(scope, &prefix, separator))
                            .or_else(|| CanonicalKey::new(&prefix, separator)),
                        binding.scope.clone(),
                        Some(prefix.clone()),
                    )
                } else {
                    (None, None, None)
                };
                if let Some(key) = key {
                    out.push(SourceOccurrence {
                        uri: uri.clone(),
                        range,
                        key,
                        namespace: resolve_binding(&callee, node.start_byte(), bindings)
                            .and_then(|binding| binding.namespace.clone())
                            .or_else(|| {
                                full_key_functions
                                    .iter()
                                    .find(|function| function.function_name == callee)
                                    .and_then(|function| function.default_namespace.clone())
                            })
                            .or_else(|| default_namespace.map(str::to_owned)),
                        kind: OccurrenceKind::DynamicScope,
                        scope,
                        relative_key,
                        arguments: None,
                    });
                }
            } else if argument.kind() != "template_string"
                && let Some(binding) = resolve_binding(&callee, node.start_byte(), bindings)
            {
                if let Some(key) = binding.scope.clone() {
                    out.push(SourceOccurrence {
                        uri: uri.clone(),
                        range: ByteRange(argument.start_byte()..argument.start_byte()),
                        key,
                        namespace: binding.namespace.clone(),
                        kind: OccurrenceKind::DynamicScope,
                        scope: None,
                        relative_key: None,
                        arguments: None,
                    });
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
            default_namespace,
            bindings,
            out,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_bindings(
    name: Node<'_>,
    text: &str,
    methods: &[String],
    namespace: String,
    scope: Option<CanonicalKey>,
    declaration_range: ByteRange,
    visibility: ByteRange,
    out: &mut Vec<ScopeBinding>,
) {
    if name.kind() == "identifier" {
        if let Ok(binding_name) = name.utf8_text(text.as_bytes()) {
            for method in methods {
                out.push(ScopeBinding {
                    name: binding_name.into(),
                    method: method.clone(),
                    scope: scope.clone(),
                    namespace: Some(namespace.clone()),
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
            if methods.iter().any(|method| method == property) {
                out.push(ScopeBinding {
                    name: local.into(),
                    method: property.into(),
                    scope: scope.clone(),
                    namespace: Some(namespace.clone()),
                    declaration_range: declaration_range.clone(),
                    visibility: visibility.clone(),
                    direct_function: true,
                });
            }
        }
    }
}

fn is_call_named(node: Node<'_>, text: &str, expected: &str) -> bool {
    node.kind() == "call_expression"
        && node
            .child_by_field_name("function")
            .and_then(|n| n.utf8_text(text.as_bytes()).ok())
            == Some(expected)
}

fn call_argument(node: Node<'_>, index: usize) -> Option<Node<'_>> {
    let args = node.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    args.named_children(&mut cursor).nth(index)
}

fn translation_arguments(call: Node<'_>, text: &str) -> TranslationArguments {
    let Some(options) = call_argument(call, 1) else {
        return TranslationArguments::default();
    };
    if options.kind() != "object" {
        return TranslationArguments {
            supplied: vec![],
            uncertain: true,
        };
    }
    let mut result = TranslationArguments::default();
    let mut cursor = options.walk();
    for child in options.named_children(&mut cursor) {
        match child.kind() {
            "pair" => {
                let Some(key) = child.child_by_field_name("key") else {
                    result.uncertain = true;
                    continue;
                };
                if !matches!(key.kind(), "property_identifier" | "identifier" | "string") {
                    result.uncertain = true;
                    continue;
                }
                let Some(name) = property_name(key, text) else {
                    result.uncertain = true;
                    continue;
                };
                result.supplied.push(TranslationArgument {
                    name: name.to_owned(),
                    range: if key.kind() == "string" {
                        content_range(key)
                    } else {
                        ByteRange(key.byte_range())
                    },
                });
            }
            "shorthand_property_identifier" => {
                if let Ok(name) = child.utf8_text(text.as_bytes()) {
                    result.supplied.push(TranslationArgument {
                        name: name.to_owned(),
                        range: ByteRange(child.byte_range()),
                    });
                } else {
                    result.uncertain = true;
                }
            }
            _ => result.uncertain = true,
        }
    }
    result
}

fn object_string_property(node: Node<'_>, text: &str, expected: &str) -> Option<String> {
    object_string_property_node(node, text, expected).and_then(|value| literal_value(value, text))
}

fn object_string_property_node<'a>(node: Node<'a>, text: &str, expected: &str) -> Option<Node<'a>> {
    if node.kind() != "object" {
        return None;
    }
    let mut cursor = node.walk();
    for pair in node
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "pair")
    {
        let key = pair
            .child_by_field_name("key")
            .and_then(|key| property_name(key, text));
        if key == Some(expected) {
            return pair.child_by_field_name("value");
        }
    }
    None
}

fn call_namespace_option(node: Node<'_>, text: &str) -> Option<String> {
    call_argument(node, 1).and_then(|options| object_string_property(options, text, "ns"))
}

fn split_namespace_key(value: &str, separator: &str) -> Option<(String, CanonicalKey)> {
    let (namespace, key) = value.split_once(':')?;
    if namespace.is_empty() {
        return None;
    }
    Some((namespace.to_owned(), CanonicalKey::new(key, separator)?))
}

fn call_with_first_argument<'a>(node: Node<'a>, text: &str) -> Option<(String, Node<'a>)> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    let callee = function.utf8_text(text.as_bytes()).ok()?.to_owned();
    let args = node.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    Some((callee, args.named_children(&mut cursor).next()?))
}

fn dynamic_template_prefix(
    node: Node<'_>,
    text: &str,
    separator: &str,
) -> Option<(String, ByteRange)> {
    if node.kind() != "template_string" {
        return None;
    }
    let mut cursor = node.walk();
    let mut children = node.named_children(&mut cursor);
    let fragment = children.next()?;
    if fragment.kind() != "string_fragment" || children.next()?.kind() != "template_substitution" {
        return None;
    }
    let raw = fragment.utf8_text(text.as_bytes()).ok()?;
    let prefix = raw.strip_suffix(separator)?;
    CanonicalKey::new(prefix, separator)?;
    Some((
        prefix.to_owned(),
        ByteRange(fragment.start_byte()..fragment.end_byte().saturating_sub(separator.len())),
    ))
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
    default_namespace: Option<&str>,
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
            namespace: default_namespace.map(str::to_owned),
            kind: OccurrenceKind::FullKey,
            scope: None,
            relative_key: None,
            arguments: None,
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
            default_namespace,
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
    let (callee, literal) = call_with_first_argument(node, text)?;
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
    fn captures_static_translation_arguments_and_uncertainty() {
        let uri = Url::parse("file:///app.ts").unwrap();
        let text = "i18next.t('items', { count, name: user, 'label': value }); i18next.t('other', { count, ...values }); i18next.t('third', options);";
        let (found, _) = analyze_source(&uri, text, ".", &[], &[], &["i18next.t".into()], &[], &[]);
        let calls = found
            .iter()
            .filter(|occurrence| occurrence.kind == OccurrenceKind::FullKey)
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 3);
        let first = calls[0].arguments.as_ref().unwrap();
        assert!(!first.uncertain);
        assert_eq!(
            first
                .supplied
                .iter()
                .map(|argument| argument.name.as_str())
                .collect::<Vec<_>>(),
            ["count", "name", "label"]
        );
        assert!(calls[1].arguments.as_ref().unwrap().uncertain);
        assert!(calls[1].arguments.as_ref().unwrap().has("count"));
        assert!(calls[2].arguments.as_ref().unwrap().uncertain);
    }

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
    fn recognizes_standard_i18next_namespaces_and_key_prefixes() {
        let uri = Url::parse("file:///app.tsx").unwrap();
        let text = r#"
          const { t: commonT } = useTranslation('common', { keyPrefix: 'buttons' });
          commonT('save');
          const i18n = useTranslation();
          i18n.t('errors:notFound');
          i18next.t('title', { ns: 'home' });
        "#;
        let (found, bindings) =
            analyze_source_with_namespace(&uri, text, ".", &[], &[], &[], &[], Some("translation"));
        assert!(bindings.iter().any(|binding| {
            binding.name == "commonT"
                && binding.namespace.as_deref() == Some("common")
                && binding
                    .scope
                    .as_ref()
                    .is_some_and(|scope| scope.as_str() == "buttons")
        }));
        assert!(found.iter().any(
            |occurrence| occurrence.namespace.as_deref() == Some("common")
                && occurrence.key.as_str() == "buttons.save"
        ));
        assert!(found.iter().any(|occurrence| {
            occurrence.kind == OccurrenceKind::ScopeDeclaration
                && occurrence.namespace.as_deref() == Some("common")
                && occurrence.key.as_str() == "buttons"
        }));
        assert!(found.iter().any(
            |occurrence| occurrence.namespace.as_deref() == Some("errors")
                && occurrence.key.as_str() == "notFound"
        ));
        assert!(
            found
                .iter()
                .any(|occurrence| occurrence.namespace.as_deref() == Some("home")
                    && occurrence.key.as_str() == "title")
        );
    }

    #[test]
    fn function_namespaces_and_methods_override_the_global_default() {
        let uri = Url::parse("file:///app.tsx").unwrap();
        let text = r#"
          const a = useAlpha('Page');
          a.t('title');
          a.key('ignored');
          const { key } = useBeta('Card');
          key('label');
          translate('plain');
          translate('explicit:value');
        "#;
        let scoped = vec![
            ScopedFunctionConfig {
                function_name: "useAlpha".into(),
                default_namespace: Some("alpha".into()),
                translation_methods: vec!["t".into()],
            },
            ScopedFunctionConfig {
                function_name: "useBeta".into(),
                default_namespace: None,
                translation_methods: vec!["key".into()],
            },
        ];
        let full = vec![FullKeyFunctionConfig {
            function_name: "translate".into(),
            default_namespace: Some("full".into()),
        }];
        let (found, bindings) = analyze_source_with_namespace(
            &uri,
            text,
            ".",
            &scoped,
            &full,
            &[],
            &[],
            Some("global"),
        );

        assert!(bindings.iter().any(|binding| {
            binding.name == "a"
                && binding.method == "t"
                && binding.namespace.as_deref() == Some("alpha")
        }));
        assert!(
            !bindings
                .iter()
                .any(|binding| binding.name == "a" && binding.method == "key")
        );
        assert!(found.iter().any(|occurrence| {
            occurrence.namespace.as_deref() == Some("alpha")
                && occurrence.key.as_str() == "Page.title"
        }));
        assert!(found.iter().any(|occurrence| {
            occurrence.namespace.as_deref() == Some("global")
                && occurrence.key.as_str() == "Card.label"
        }));
        assert!(found.iter().any(|occurrence| {
            occurrence.namespace.as_deref() == Some("full") && occurrence.key.as_str() == "plain"
        }));
        assert!(found.iter().any(|occurrence| {
            occurrence.namespace.as_deref() == Some("explicit")
                && occurrence.key.as_str() == "value"
        }));
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
    fn recognizes_dynamic_template_scopes() {
        let uri = Url::parse("file:///app.ts").unwrap();
        let text = "const i18n=useScopedTranslation('Page'); i18n.t(`Cards.${card}`); i18next.t(`SomeScope.${value}.ignored`)";
        let (found, _) = analyze_source(
            &uri,
            text,
            ".",
            &["useScopedTranslation".into()],
            &["t".into()],
            &["i18next.t".into()],
            &[],
            &[],
        );
        let dynamic = found
            .iter()
            .filter(|occurrence| occurrence.kind == OccurrenceKind::DynamicScope)
            .map(|occurrence| occurrence.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(dynamic, ["Page.Cards", "SomeScope"]);
        assert_eq!(&text[found[1].range.0.clone()], "Cards");
        assert_eq!(found[1].scope.as_ref().unwrap().as_str(), "Page");
        assert_eq!(found[1].relative_key.as_deref(), Some("Cards"));
    }

    #[test]
    fn non_literal_scoped_calls_mark_the_binding_scope_dynamic() {
        let uri = Url::parse("file:///app.ts").unwrap();
        let text = "const i18n=useScopedTranslation('AiCaseStatuses'); i18n.t(testCase.status)";
        let (found, _) = analyze_source(
            &uri,
            text,
            ".",
            &["useScopedTranslation".into()],
            &["t".into()],
            &["i18next.t".into()],
            &[],
            &[],
        );
        let dynamic = found
            .iter()
            .find(|occurrence| occurrence.kind == OccurrenceKind::DynamicScope)
            .unwrap();
        assert_eq!(dynamic.key.as_str(), "AiCaseStatuses");
        assert!(dynamic.range.0.is_empty());
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
