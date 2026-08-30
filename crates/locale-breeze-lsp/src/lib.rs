use anyhow::Result;
use locale_breeze_core::{
    ByteRange, CanonicalKey, EntryKind, IndexSnapshot, LineIndex, OccurrenceKind, WorkspaceIndex,
};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::*;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use url::Url;

const RESOLVE_KEY_COMMAND: &str = "localeBreeze.resolveFullKey";
const REFRESH_DOCUMENT_COMMAND: &str = "localeBreeze.refreshDocument";

pub fn run_stdio(config_override: Option<PathBuf>) -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(true),
                })),
                ..Default::default()
            },
        )),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec!["\"".into(), "'".into(), ".".into()]),
            ..Default::default()
        }),
        definition_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![RESOLVE_KEY_COMMAND.into(), REFRESH_DOCUMENT_COMMAND.into()],
            ..Default::default()
        }),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            file_operations: None,
        }),
        ..Default::default()
    };
    let params: InitializeParams =
        serde_json::from_value(connection.initialize(serde_json::to_value(capabilities)?)?)?;
    let mut server = Server::new(config_override);
    server.initialize(&connection, &params);
    server.event_loop(&connection)?;
    io_threads.join()?;
    Ok(())
}

struct Server {
    workspaces: Vec<Arc<WorkspaceIndex>>,
    watchers: Vec<RecommendedWatcher>,
    config_override: Option<PathBuf>,
    published_diagnostics: Arc<Mutex<HashMap<Url, Vec<Diagnostic>>>>,
}

impl Server {
    fn new(config_override: Option<PathBuf>) -> Self {
        Self {
            workspaces: vec![],
            watchers: vec![],
            config_override,
            published_diagnostics: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[allow(deprecated)]
    fn initialize(&mut self, connection: &Connection, params: &InitializeParams) {
        let roots: Vec<Url> = params
            .workspace_folders
            .as_ref()
            .map(|folders| folders.iter().map(|f| f.uri.clone()).collect())
            .or_else(|| params.root_uri.clone().map(|u| vec![u]))
            .unwrap_or_default();
        for root in roots {
            self.add_workspace(connection, root);
        }
    }

    fn add_workspace(&mut self, connection: &Connection, uri: Url) {
        let Ok(root) = uri.to_file_path() else { return };
        if self.workspaces.iter().any(|w| same_path(w.root(), &root)) {
            return;
        }
        let config_path = self
            .config_override
            .clone()
            .unwrap_or_else(|| root.join("locale-breeze.json"));
        match WorkspaceIndex::load(root.clone(), &config_path) {
            Ok(workspace) => {
                let workspace = Arc::new(workspace);
                let watched = workspace.clone();
                let sender = connection.sender.clone();
                let published_diagnostics = self.published_diagnostics.clone();
                match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if let Ok(event) = event {
                        for path in event.paths {
                            watched.refresh_disk_path(&path);
                        }
                        for notification in
                            diagnostic_notifications(&watched, &published_diagnostics)
                        {
                            if let Some(trace) = diagnostic_trace_notification(&notification) {
                                let _ = sender.send(Message::Notification(trace));
                            }
                            let _ = sender.send(Message::Notification(notification));
                        }
                    }
                }) {
                    Ok(mut watcher) => {
                        if watcher.watch(&root, RecursiveMode::Recursive).is_ok() {
                            self.watchers.push(watcher);
                        }
                    }
                    Err(error) => log(
                        connection,
                        MessageType::WARNING,
                        format!("LocaleBreeze could not watch {}: {error}", root.display()),
                    ),
                }
                log(
                    connection,
                    MessageType::INFO,
                    format!(
                        "LocaleBreeze server pid={} indexed {}",
                        std::process::id(),
                        root.display()
                    ),
                );
                self.workspaces.push(workspace);
                self.workspaces
                    .sort_by_key(|w| std::cmp::Reverse(w.root().components().count()));
                if let Some(workspace) = self.workspace_for_uri(&uri) {
                    publish_diagnostics(connection, workspace, &self.published_diagnostics);
                }
            }
            Err(error) => log(
                connection,
                MessageType::ERROR,
                format!("LocaleBreeze disabled for {}: {error}", root.display()),
            ),
        }
    }

    fn workspace_for_uri(&self, uri: &Url) -> Option<&Arc<WorkspaceIndex>> {
        let path = uri.to_file_path().ok()?;
        self.workspaces
            .iter()
            .find(|workspace| path_is_within(&path, workspace.root()))
    }

    fn reload_workspaces(&mut self, connection: &Connection) {
        let roots: Vec<_> = self
            .workspaces
            .iter()
            .filter_map(|workspace| Url::from_file_path(workspace.root()).ok())
            .collect();
        for workspace in &self.workspaces {
            clear_diagnostics(connection, workspace, &self.published_diagnostics);
        }
        self.workspaces.clear();
        self.watchers.clear();
        for root in roots {
            self.add_workspace(connection, root);
        }
    }

    fn event_loop(&mut self, connection: &Connection) -> Result<()> {
        for message in &connection.receiver {
            match message {
                Message::Request(request) => {
                    if connection.handle_shutdown(&request)? {
                        return Ok(());
                    }
                    let response = self.handle_request(request);
                    connection.sender.send(Message::Response(response))?;
                }
                Message::Notification(notification) => {
                    self.handle_notification(connection, notification)
                }
                Message::Response(_) => {}
            }
        }
        Ok(())
    }

    fn handle_request(&self, request: Request) -> Response {
        let id = request.id.clone();
        let result = match request.method.as_str() {
            "textDocument/completion" => parse::<CompletionParams>(request.params)
                .and_then(|p| serialize_optional(self.completion(p)?)),
            "textDocument/definition" => parse::<GotoDefinitionParams>(request.params)
                .and_then(|p| serialize_optional(self.definition(p)?)),
            "textDocument/hover" => parse::<HoverParams>(request.params)
                .and_then(|p| serialize_optional(self.hover(p)?)),
            "textDocument/references" => parse::<ReferenceParams>(request.params)
                .and_then(|p| serialize_optional(self.references(p)?)),
            "workspace/executeCommand" => parse::<ExecuteCommandParams>(request.params)
                .and_then(|p| serialize_optional(self.execute_command(p)?)),
            _ => {
                return Response::new_err(
                    id,
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("unsupported request: {}", request.method),
                );
            }
        };
        match result {
            Ok(Some(value)) => Response::new_ok(id, value),
            Ok(None) => Response::new_ok(id, Value::Null),
            Err(error) => Response::new_err(
                id,
                lsp_server::ErrorCode::InvalidParams as i32,
                error.to_string(),
            ),
        }
    }

    #[allow(clippy::collapsible_if)]
    fn handle_notification(&mut self, connection: &Connection, notification: Notification) {
        match notification.method.as_str() {
            "textDocument/didOpen" => {
                if let Ok(p) = parse::<DidOpenTextDocumentParams>(notification.params) {
                    if let Some(w) = self.workspace_for_uri(&p.text_document.uri) {
                        w.update_text(
                            p.text_document.uri,
                            p.text_document.text,
                            Some(p.text_document.version),
                        );
                        publish_diagnostics(connection, w, &self.published_diagnostics);
                    }
                }
            }
            "textDocument/didChange" => {
                if let Ok(p) = parse::<DidChangeTextDocumentParams>(notification.params) {
                    if let Some(w) = self.workspace_for_uri(&p.text_document.uri) {
                        let snapshot = w.snapshot();
                        if let Some(text) = snapshot.text(&p.text_document.uri) {
                            let updated = apply_changes(text.to_owned(), &p.content_changes);
                            w.update_text(
                                p.text_document.uri,
                                updated,
                                Some(p.text_document.version),
                            );
                            publish_diagnostics(connection, w, &self.published_diagnostics);
                        }
                    }
                }
            }
            "textDocument/didClose" => {
                if let Ok(p) = parse::<DidCloseTextDocumentParams>(notification.params) {
                    if let Some(w) = self.workspace_for_uri(&p.text_document.uri) {
                        w.close_document(&p.text_document.uri);
                        publish_diagnostics(connection, w, &self.published_diagnostics);
                    }
                }
            }
            "textDocument/didSave" => {
                if let Ok(p) = parse::<DidSaveTextDocumentParams>(notification.params)
                    && let Some(text) = p.text
                    && let Some(w) = self.workspace_for_uri(&p.text_document.uri)
                {
                    w.update_text(p.text_document.uri, text, None);
                    publish_diagnostics(connection, w, &self.published_diagnostics);
                }
            }
            "workspace/didChangeWorkspaceFolders" => {
                if let Ok(p) = parse::<DidChangeWorkspaceFoldersParams>(notification.params) {
                    for removed in p.event.removed {
                        if let Ok(path) = removed.uri.to_file_path() {
                            for workspace in self
                                .workspaces
                                .iter()
                                .filter(|w| same_path(w.root(), &path))
                            {
                                clear_diagnostics(
                                    connection,
                                    workspace,
                                    &self.published_diagnostics,
                                );
                            }
                            self.workspaces
                                .retain(|workspace| !same_path(workspace.root(), &path));
                        }
                    }
                    for added in p.event.added {
                        self.add_workspace(connection, added.uri);
                    }
                }
            }
            "workspace/didChangeConfiguration" => {
                self.reload_workspaces(connection);
            }
            "workspace/didChangeWatchedFiles" => {
                if let Ok(params) = parse::<DidChangeWatchedFilesParams>(notification.params)
                    && params.changes.iter().any(|change| {
                        change
                            .uri
                            .path_segments()
                            .and_then(Iterator::last)
                            .is_some_and(|name| name == "locale-breeze.json")
                    })
                {
                    self.reload_workspaces(connection);
                }
            }
            _ => {}
        }
    }

    fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(workspace) = self.workspace_for_uri(&uri) else {
            return Ok(None);
        };
        let snapshot = workspace.snapshot();
        let Some(offset) = position_offset(&snapshot, &uri, position) else {
            return Ok(None);
        };
        let Some(context) = snapshot.completion_context_at(&uri, offset, workspace.config()) else {
            return Ok(None);
        };
        let (candidates, incomplete) = snapshot.completions(
            &context,
            &workspace.config().default_locale,
            &workspace.config().key_separator,
            100,
        );
        let items = candidates
            .into_iter()
            .map(|candidate| CompletionItem {
                label: match &candidate.detail {
                    Some(value) => format!("{} — {}", candidate.key, value),
                    None => candidate.key.clone(),
                },
                kind: Some(CompletionItemKind::PROPERTY),
                detail: Some(candidate.canonical_key),
                filter_text: Some(candidate.key.clone()),
                insert_text: Some(candidate.key),
                // Keep LocaleBreeze's results ahead of suggestions whose sort text is
                // derived from their label, while preserving relevance within our list.
                sort_text: Some(format!(
                    "00000-{:05}",
                    10_000i64.saturating_sub(candidate.score)
                )),
                ..Default::default()
            })
            .collect();
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: incomplete,
            items,
        })))
    }

    fn definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(workspace) = self.workspace_for_uri(&uri) else {
            return Ok(None);
        };
        let snapshot = workspace.snapshot();
        let Some(offset) = position_offset(&snapshot, &uri, position) else {
            return Ok(None);
        };
        let Some(key) =
            key_at_position(&snapshot, &uri, position, &workspace.config().key_separator)
        else {
            return Ok(None);
        };
        let locations = if snapshot.dictionary_at(&uri, offset).is_some() {
            let is_scope = snapshot
                .dictionary_entries(&key)
                .iter()
                .any(|entry| entry.kind == EntryKind::Object);
            let occurrences: Vec<_> = if is_scope {
                snapshot.scope_occurrences(&key, &workspace.config().key_separator, 32)
            } else {
                snapshot.occurrences(&key).iter().collect()
            };
            occurrences
                .into_iter()
                .filter_map(|occurrence| location(&snapshot, &occurrence.uri, &occurrence.range))
                .collect::<Vec<_>>()
        } else {
            snapshot
                .dictionary_entries(&key)
                .iter()
                .filter(|entry| entry.locale == workspace.config().default_locale)
                .filter_map(|entry| location(&snapshot, &entry.uri, &entry.key_range))
                .collect::<Vec<_>>()
        };
        Ok((!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations)))
    }

    fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(workspace) = self.workspace_for_uri(&uri) else {
            return Ok(None);
        };
        let snapshot = workspace.snapshot();
        let Some(key) =
            key_at_position(&snapshot, &uri, position, &workspace.config().key_separator)
        else {
            return Ok(None);
        };
        let default_locale = &workspace.config().default_locale;
        let default_entry = snapshot
            .dictionary_entries(&key)
            .iter()
            .find(|entry| entry.locale == *default_locale);
        let text = match default_entry {
            Some(entry) if entry.kind == EntryKind::Leaf => entry.value.clone().unwrap_or_default(),
            Some(entry) if entry.kind == EntryKind::Object => {
                let mut children = snapshot
                    .direct_dictionary_children(&key, &workspace.config().key_separator)
                    .filter(|child| child.locale == *default_locale)
                    .collect::<Vec<_>>();
                children.sort_by(|left, right| {
                    left.uri
                        .as_str()
                        .cmp(right.uri.as_str())
                        .then(left.key_range.0.start.cmp(&right.key_range.0.start))
                });
                let mut seen = HashSet::new();
                children.retain(|child| seen.insert(child.key.clone()));
                let has_more = children.len() > 5;
                let mut preview = children
                    .into_iter()
                    .take(5)
                    .map(|child| {
                        let relative = child
                            .key
                            .relative_to(&key, &workspace.config().key_separator)
                            .unwrap_or(child.key.as_str());
                        match child.value.as_deref() {
                            Some(value) => format!("- `{relative}`: {value}"),
                            None => format!("- `{relative}`"),
                        }
                    })
                    .collect::<Vec<_>>();
                if has_more {
                    preview.push("...".into());
                }
                preview.join("\n")
            }
            _ => format!("Translation key `{}` does not exist.", key.as_str()),
        };
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: text,
            }),
            range: None,
        }))
    }

    fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(workspace) = self.workspace_for_uri(&uri) else {
            return Ok(None);
        };
        let snapshot = workspace.snapshot();
        let Some(key) =
            key_at_position(&snapshot, &uri, position, &workspace.config().key_separator)
        else {
            return Ok(None);
        };
        let is_scope = snapshot
            .dictionary_entries(&key)
            .iter()
            .any(|e| e.kind == EntryKind::Object)
            || snapshot
                .occurrence_at(
                    &uri,
                    position_offset(&snapshot, &uri, position).unwrap_or(usize::MAX),
                )
                .is_some_and(|o| o.kind == OccurrenceKind::ScopeDeclaration);
        let occurrences: Vec<_> = if is_scope {
            snapshot.scope_occurrences(&key, &workspace.config().key_separator, 32)
        } else {
            snapshot.occurrences(&key).iter().collect()
        };
        let mut locations: Vec<_> = occurrences
            .into_iter()
            .filter_map(|o| location(&snapshot, &o.uri, &o.range))
            .collect();
        if params.context.include_declaration {
            locations.extend(
                snapshot
                    .dictionary_entries(&key)
                    .iter()
                    .filter_map(|e| location(&snapshot, &e.uri, &e.key_range)),
            );
        }
        Ok(Some(locations))
    }

    fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<String>> {
        match params.command.as_str() {
            RESOLVE_KEY_COMMAND => {
                let Some(argument) = params.arguments.first() else {
                    return Ok(None);
                };
                let position: TextDocumentPositionParams =
                    serde_json::from_value(argument.clone())?;
                let uri = position.text_document.uri;
                let Some(workspace) = self.workspace_for_uri(&uri) else {
                    return Ok(None);
                };
                let snapshot = workspace.snapshot();
                Ok(key_at_position(
                    &snapshot,
                    &uri,
                    position.position,
                    &workspace.config().key_separator,
                )
                .map(|key| key.as_str().to_owned()))
            }
            REFRESH_DOCUMENT_COMMAND => {
                let Some(uri) = params.arguments.first() else {
                    return Ok(None);
                };
                let Some(text) = params.arguments.get(1) else {
                    return Ok(None);
                };
                let uri: Url = serde_json::from_value(uri.clone())?;
                let text: String = serde_json::from_value(text.clone())?;
                if let Some(workspace) = self.workspace_for_uri(&uri) {
                    // No version is intentional: recovery must replace a stale
                    // versioned snapshot with the editor's current full text.
                    workspace.update_text(uri, text, None);
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T> {
    Ok(serde_json::from_value(value)?)
}

fn serialize_optional<T: serde::Serialize>(value: Option<T>) -> Result<Option<Value>> {
    value
        .map(serde_json::to_value)
        .transpose()
        .map_err(Into::into)
}

fn position_offset(snapshot: &IndexSnapshot, uri: &Url, position: Position) -> Option<usize> {
    let text = snapshot.text(uri)?;
    LineIndex::new(text).offset(text, position.line, position.character)
}

fn key_at_position(
    snapshot: &IndexSnapshot,
    uri: &Url,
    position: Position,
    separator: &str,
) -> Option<CanonicalKey> {
    let offset = position_offset(snapshot, uri, position)?;
    snapshot
        .occurrence_at(uri, offset)
        .and_then(|occurrence| {
            let text = snapshot.text(uri)?;
            let literal = text.get(occurrence.range.0.clone())?;
            let cursor = offset
                .saturating_sub(occurrence.range.0.start)
                .min(literal.len());
            let segment_index = literal.get(..cursor)?.matches(separator).count();

            let (scope, key) = match (&occurrence.scope, &occurrence.relative_key) {
                (Some(scope), Some(relative)) => (Some(scope), relative.as_str()),
                _ => (None, occurrence.key.as_str()),
            };
            let prefix = key
                .split(separator)
                .take(segment_index + 1)
                .collect::<Vec<_>>()
                .join(separator);
            match scope {
                Some(scope) => CanonicalKey::join(scope, &prefix, separator),
                None => CanonicalKey::new(prefix, separator),
            }
        })
        .or_else(|| snapshot.dictionary_at(uri, offset).map(|e| e.key.clone()))
}

fn location(snapshot: &IndexSnapshot, uri: &Url, range: &ByteRange) -> Option<Location> {
    let text = snapshot.text(uri)?;
    let index = LineIndex::new(text);
    let (sl, sc) = index.position(text, range.0.start)?;
    let (el, ec) = index.position(text, range.0.end)?;
    Some(Location::new(
        uri.clone(),
        Range::new(Position::new(sl, sc), Position::new(el, ec)),
    ))
}

fn apply_changes(mut text: String, changes: &[TextDocumentContentChangeEvent]) -> String {
    for change in changes {
        if let Some(range) = change.range {
            let index = LineIndex::new(&text);
            if let (Some(start), Some(end)) = (
                index.offset(&text, range.start.line, range.start.character),
                index.offset(&text, range.end.line, range.end.character),
            ) {
                text.replace_range(start..end, &change.text);
            }
        } else {
            text = change.text.clone();
        }
    }
    text
}

#[cfg(windows)]
fn same_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn same_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right
}

fn path_is_within(path: &std::path::Path, root: &std::path::Path) -> bool {
    let mut path_components = path.components();
    root.components().all(|root_component| {
        path_components.next().is_some_and(|path_component| {
            #[cfg(windows)]
            {
                path_component
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&root_component.as_os_str().to_string_lossy())
            }
            #[cfg(not(windows))]
            {
                path_component == root_component
            }
        })
    })
}

fn diagnostic_notifications(
    workspace: &WorkspaceIndex,
    published: &Mutex<HashMap<Url, Vec<Diagnostic>>>,
) -> Vec<Notification> {
    let snapshot = workspace.snapshot();
    let mut by_uri: HashMap<Url, Vec<Diagnostic>> = HashMap::new();
    for entry in snapshot.default_locale_leaf_entries(&workspace.config().default_locale) {
        let diagnostics = by_uri.entry(entry.uri.clone()).or_default();
        if workspace.config().unused_keys
            && !snapshot.is_leaf_key_used(&entry.key)
            && let Some(range) = location(&snapshot, &entry.uri, &entry.key_range).map(|l| l.range)
        {
            diagnostics.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::WARNING),
                code: None,
                code_description: None,
                source: Some("locale-breeze".into()),
                message: format!("Translation key \"{}\" seems unused", entry.key),
                related_information: None,
                tags: Some(vec![DiagnosticTag::UNNECESSARY]),
                data: None,
            });
        }
    }
    let mut cache = published
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut uris = by_uri.keys().cloned().collect::<Vec<_>>();
    uris.extend(cache.keys().filter_map(|uri| {
        uri.to_file_path()
            .ok()
            .filter(|path| path_is_within(path, workspace.root()))
            .map(|_| uri.clone())
    }));
    uris.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    uris.dedup();

    uris.into_iter()
        .filter_map(|uri| {
            let diagnostics = by_uri.remove(&uri).unwrap_or_default();
            if cache.get(&uri) == Some(&diagnostics) {
                return None;
            }
            cache.insert(uri.clone(), diagnostics.clone());
            Notification::new(
                "textDocument/publishDiagnostics".into(),
                PublishDiagnosticsParams::new(uri, diagnostics, None),
            )
            .into()
        })
        .collect()
}

fn publish_diagnostics(
    connection: &Connection,
    workspace: &WorkspaceIndex,
    published: &Mutex<HashMap<Url, Vec<Diagnostic>>>,
) {
    for notification in diagnostic_notifications(workspace, published) {
        if let Some(trace) = diagnostic_trace_notification(&notification) {
            let _ = connection.sender.send(Message::Notification(trace));
        }
        let _ = connection.sender.send(Message::Notification(notification));
    }
}

fn diagnostic_trace_notification(notification: &Notification) -> Option<Notification> {
    let params =
        serde_json::from_value::<PublishDiagnosticsParams>(notification.params.clone()).ok()?;
    let entries = params
        .diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{}:{}-{}:{} {}",
                diagnostic.range.start.line,
                diagnostic.range.start.character,
                diagnostic.range.end.line,
                diagnostic.range.end.character,
                diagnostic.message
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let message = format!(
        "LocaleBreeze diagnostics pid={} uri={} count={} [{}]",
        std::process::id(),
        params.uri,
        params.diagnostics.len(),
        entries
    );
    Some(Notification::new(
        "window/logMessage".into(),
        LogMessageParams {
            typ: MessageType::INFO,
            message,
        },
    ))
}

fn clear_diagnostics(
    connection: &Connection,
    workspace: &WorkspaceIndex,
    published: &Mutex<HashMap<Url, Vec<Diagnostic>>>,
) {
    let snapshot = workspace.snapshot();
    let mut uris = snapshot
        .default_locale_leaf_entries(&workspace.config().default_locale)
        .map(|entry| entry.uri.clone())
        .collect::<Vec<_>>();
    let mut cache = published
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    uris.extend(cache.keys().filter_map(|uri| {
        uri.to_file_path()
            .ok()
            .filter(|path| path_is_within(path, workspace.root()))
            .map(|_| uri.clone())
    }));
    uris.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    uris.dedup();
    for uri in uris {
        cache.remove(&uri);
        let notification = Notification::new(
            "textDocument/publishDiagnostics".into(),
            PublishDiagnosticsParams::new(uri, Vec::new(), None),
        );
        let _ = connection.sender.send(Message::Notification(notification));
    }
}

fn log(connection: &Connection, typ: MessageType, message: String) {
    let params = LogMessageParams { typ, message };
    let notification = Notification::new("window/logMessage".into(), params);
    let _ = connection.sender.send(Message::Notification(notification));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn workspace_path_matching_ignores_windows_casing() {
        let root = std::path::Path::new(r"C:\Users\Example\Project");
        let file = std::path::Path::new(r"c:\users\example\project\src\app.ts");
        assert!(same_path(
            root,
            std::path::Path::new(r"c:\users\example\project")
        ));
        assert!(path_is_within(file, root));
    }

    #[test]
    fn applies_utf16_incremental_edits() {
        let text = "const x = '😀';".to_owned();
        let changed = apply_changes(
            text,
            &[TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 11), Position::new(0, 13))),
                range_length: Some(2),
                text: "ok".into(),
            }],
        );
        assert_eq!(changed, "const x = 'ok';");
    }

    #[test]
    fn suppresses_duplicate_unused_key_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "keySeparator":".",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"],
              "unusedKeys":true
            }"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("translation.en.json"),
            r#"{"my_key":"Value"}"#,
        )
        .unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let published = Mutex::new(HashMap::new());

        let first = diagnostic_notifications(&workspace, &published);
        assert_eq!(first.len(), 1);
        let params: PublishDiagnosticsParams =
            serde_json::from_value(first[0].params.clone()).unwrap();
        assert_eq!(params.diagnostics.len(), 1);
        assert_eq!(
            params.diagnostics[0].severity,
            Some(DiagnosticSeverity::WARNING)
        );
        assert_eq!(
            params.diagnostics[0].message,
            "Translation key \"my_key\" seems unused"
        );
        assert_eq!(
            params.diagnostics[0].tags,
            Some(vec![DiagnosticTag::UNNECESSARY])
        );
        assert!(diagnostic_notifications(&workspace, &published).is_empty());

        #[cfg(windows)]
        {
            let canonical = params.uri.as_str().strip_prefix("file:///").unwrap();
            let (drive, rest) = canonical.split_once(':').unwrap();
            let webstorm_uri = Url::parse(&format!(
                "file:///{}%3A{}",
                drive.to_ascii_lowercase(),
                rest
            ))
            .unwrap();
            workspace.update_text(webstorm_uri, r#"{"my_key":"Value"}"#.into(), Some(1));
            assert!(diagnostic_notifications(&workspace, &published).is_empty());
        }
    }

    #[test]
    fn hover_and_dictionary_navigation_recover_from_stale_index() {
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
        let dictionary_text = r#"{
              "known":"<strong>Default value</strong>",
              "scope":{"c":"C","a":"A","f":"F","b":"B","e":"E","d":"D"}
            }"#;
        std::fs::write(temp.path().join("translation.en.json"), dictionary_text).unwrap();
        let source_path = temp.path().join("app.ts");
        std::fs::write(
            &source_path,
            "i18next.t('known');\ni18next.t('missing');\nuseScopedTranslation('scope');",
        )
        .unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let uri = Url::from_file_path(source_path).unwrap();
        let workspace = Arc::new(workspace);
        let server = Server {
            workspaces: vec![workspace.clone()],
            watchers: vec![],
            config_override: None,
            published_diagnostics: Default::default(),
        };
        let hover_at = |line, character| {
            server
                .hover(HoverParams {
                    text_document_position_params: TextDocumentPositionParams::new(
                        TextDocumentIdentifier::new(uri.clone()),
                        Position::new(line, character),
                    ),
                    work_done_progress_params: Default::default(),
                })
                .unwrap()
                .unwrap()
        };

        let HoverContents::Markup(found) = hover_at(0, 13).contents else {
            panic!("expected Markdown hover content");
        };
        assert_eq!(found.kind, MarkupKind::Markdown);
        assert_eq!(found.value, "<strong>Default value</strong>");

        let HoverContents::Markup(missing) = hover_at(1, 13).contents else {
            panic!("expected Markdown hover content");
        };
        assert_eq!(missing.value, "Translation key `missing` does not exist.");

        let HoverContents::Markup(scope) = hover_at(2, 23).contents else {
            panic!("expected Markdown hover content");
        };
        assert_eq!(
            scope.value,
            "- `c`: C\n- `a`: A\n- `f`: F\n- `b`: B\n- `e`: E\n..."
        );

        let dictionary_uri = Url::from_file_path(temp.path().join("translation.en.json")).unwrap();
        let definition_at_dictionary = || {
            server
                .definition(GotoDefinitionParams {
                    text_document_position_params: TextDocumentPositionParams::new(
                        TextDocumentIdentifier::new(dictionary_uri.clone()),
                        Position::new(1, 16),
                    ),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                })
                .unwrap()
        };
        let definition = definition_at_dictionary().unwrap();
        let GotoDefinitionResponse::Array(locations) = definition else {
            panic!("expected definition locations");
        };
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].uri, uri);
        assert_eq!(locations[0].range.start.line, 0);

        workspace.update_text(dictionary_uri.clone(), "{".into(), Some(99));
        assert!(definition_at_dictionary().is_none());
        server
            .execute_command(ExecuteCommandParams {
                command: REFRESH_DOCUMENT_COMMAND.into(),
                arguments: vec![
                    serde_json::to_value(&dictionary_uri).unwrap(),
                    serde_json::to_value(dictionary_text).unwrap(),
                ],
                work_done_progress_params: Default::default(),
            })
            .unwrap();
        assert!(definition_at_dictionary().is_some());
    }

    #[test]
    fn copy_key_command_returns_the_configured_full_dictionary_path() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "keySeparator":"/",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"]
            }"#,
        )
        .unwrap();
        let dictionary_path = temp.path().join("translation.en.json");
        std::fs::write(
            &dictionary_path,
            r#"{"Page":{"Login":{"submit":"Sign in"}}}"#,
        )
        .unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let uri = Url::from_file_path(dictionary_path).unwrap();
        let server = Server {
            workspaces: vec![Arc::new(workspace)],
            watchers: vec![],
            config_override: None,
            published_diagnostics: Default::default(),
        };
        let result = server
            .execute_command(ExecuteCommandParams {
                command: RESOLVE_KEY_COMMAND.into(),
                arguments: vec![
                    serde_json::to_value(TextDocumentPositionParams::new(
                        TextDocumentIdentifier::new(uri),
                        Position::new(0, 20),
                    ))
                    .unwrap(),
                ],
                work_done_progress_params: Default::default(),
            })
            .unwrap();
        assert_eq!(result.as_deref(), Some("Page/Login/submit"));
    }
}
