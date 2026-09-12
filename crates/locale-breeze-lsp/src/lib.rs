use anyhow::Result;
use locale_breeze_core::{
    ByteRange, CanonicalKey, EntryKind, IndexSnapshot, LineIndex, OccurrenceKind, WorkspaceIndex,
};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::*;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use url::Url;

const RESOLVE_KEY_COMMAND: &str = "localeBreeze.resolveFullKey";
const REFRESH_DOCUMENT_COMMAND: &str = "localeBreeze.refreshDocument";
const DOCUMENT_KEYS_COMMAND: &str = "localeBreeze.documentKeys";
const PREPARE_ADD_KEY_COMMAND: &str = "localeBreeze.prepareAddKey";

pub fn run_stdio(
    config_override: Option<PathBuf>,
    unused_keys_override: Option<bool>,
) -> Result<()> {
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
            commands: vec![
                RESOLVE_KEY_COMMAND.into(),
                REFRESH_DOCUMENT_COMMAND.into(),
                DOCUMENT_KEYS_COMMAND.into(),
                PREPARE_ADD_KEY_COMMAND.into(),
            ],
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
    let mut server = Server::new(config_override, unused_keys_override);
    server.initialize(&connection, &params);
    server.event_loop(&connection)?;
    io_threads.join()?;
    Ok(())
}

struct Server {
    workspaces: Vec<Arc<WorkspaceIndex>>,
    watchers: Vec<RecommendedWatcher>,
    config_override: Option<PathBuf>,
    unused_keys_override: Option<bool>,
    published_diagnostics: Arc<Mutex<HashMap<Url, Vec<Diagnostic>>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentKeyInfo {
    range: Range,
    key: String,
    declaration_exists: bool,
    can_add: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentKeysResult {
    version: Option<i32>,
    keys: Vec<DocumentKeyInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareAddKeyParams {
    text_document: TextDocumentIdentifier,
    position: Position,
    value: String,
    version: Option<i32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedEdit {
    uri: Url,
    version: Option<i32>,
    range: Range,
    new_text: String,
}

impl Server {
    fn new(config_override: Option<PathBuf>, unused_keys_override: Option<bool>) -> Self {
        Self {
            workspaces: vec![],
            watchers: vec![],
            config_override,
            unused_keys_override,
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
        match WorkspaceIndex::load_with_unused_override(
            root.clone(),
            &config_path,
            self.unused_keys_override,
        ) {
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
                snapshot
                    .occurrences(&key)
                    .iter()
                    .chain(
                        snapshot.dynamic_scope_occurrences(&key, &workspace.config().key_separator),
                    )
                    .collect()
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
            snapshot
                .occurrences(&key)
                .iter()
                .chain(snapshot.dynamic_scope_occurrences(&key, &workspace.config().key_separator))
                .collect()
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

    fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
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
                .map(|key| Value::String(key.as_str().to_owned())))
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
            DOCUMENT_KEYS_COMMAND => {
                let Some(argument) = params.arguments.first() else {
                    return Ok(None);
                };
                let document: TextDocumentIdentifier = serde_json::from_value(argument.clone())?;
                let Some(workspace) = self.workspace_for_uri(&document.uri) else {
                    return Ok(None);
                };
                let snapshot = workspace.snapshot();
                let default_locale = &workspace.config().default_locale;
                let keys = snapshot
                    .source_occurrences(&document.uri)
                    .iter()
                    .filter(|occurrence| !occurrence.range.0.is_empty())
                    .filter_map(|occurrence| {
                        let range = location(&snapshot, &occurrence.uri, &occurrence.range)?.range;
                        let declaration_exists = snapshot
                            .dictionary_entries(&occurrence.key)
                            .iter()
                            .any(|entry| entry.locale == *default_locale);
                        let can_add = matches!(
                            occurrence.kind,
                            OccurrenceKind::FullKey | OccurrenceKind::ScopedKey
                        ) && !declaration_exists
                            && insertion_target(
                                &snapshot,
                                &occurrence.key,
                                default_locale,
                                &workspace.config().key_separator,
                            )
                            .is_some();
                        Some(DocumentKeyInfo {
                            range,
                            key: occurrence.key.as_str().to_owned(),
                            declaration_exists,
                            can_add,
                        })
                    })
                    .collect();
                Ok(Some(serde_json::to_value(DocumentKeysResult {
                    version: snapshot.version(&document.uri),
                    keys,
                })?))
            }
            PREPARE_ADD_KEY_COMMAND => {
                let Some(argument) = params.arguments.first() else {
                    return Ok(None);
                };
                let request: PrepareAddKeyParams = serde_json::from_value(argument.clone())?;
                let Some(workspace) = self.workspace_for_uri(&request.text_document.uri) else {
                    return Ok(None);
                };
                let snapshot = workspace.snapshot();
                if request.version.is_some()
                    && request.version != snapshot.version(&request.text_document.uri)
                {
                    return Ok(None);
                }
                let Some(offset) =
                    position_offset(&snapshot, &request.text_document.uri, request.position)
                else {
                    return Ok(None);
                };
                let Some(occurrence) = snapshot.occurrence_at(&request.text_document.uri, offset)
                else {
                    return Ok(None);
                };
                if occurrence.kind == OccurrenceKind::DynamicScope
                    || snapshot
                        .dictionary_entries(&occurrence.key)
                        .iter()
                        .any(|entry| entry.locale == workspace.config().default_locale)
                {
                    return Ok(None);
                }
                let Some(edit) = prepare_insertion(
                    &snapshot,
                    &occurrence.key,
                    &workspace.config().default_locale,
                    &workspace.config().key_separator,
                    &request.value,
                ) else {
                    return Ok(None);
                };
                Ok(Some(serde_json::to_value(edit)?))
            }
            _ => Ok(None),
        }
    }
}

fn insertion_target(
    snapshot: &IndexSnapshot,
    key: &CanonicalKey,
    locale: &str,
    separator: &str,
) -> Option<(Url, Option<locale_breeze_core::DictionaryEntry>)> {
    let ancestors =
        std::iter::successors(key.parent(separator), |current| current.parent(separator))
            .collect::<Vec<_>>();
    if ancestors.iter().any(|ancestor| {
        snapshot
            .dictionary_entries(ancestor)
            .iter()
            .any(|entry| entry.locale == locale && entry.kind == EntryKind::Leaf)
    }) {
        return None;
    }
    let mut files = snapshot
        .dictionary_entries_all()
        .filter(|entry| entry.locale == locale)
        .map(|entry| entry.uri.clone())
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    files.dedup();
    let mut candidates = files
        .into_iter()
        .map(|uri| {
            let ancestor = ancestors.iter().find_map(|ancestor| {
                snapshot
                    .dictionary_entries(ancestor)
                    .iter()
                    .find(|entry| {
                        entry.locale == locale
                            && entry.uri == uri
                            && entry.kind == EntryKind::Object
                    })
                    .cloned()
            });
            let depth = ancestor
                .as_ref()
                .map_or(0, |entry| entry.key.as_str().matches(separator).count() + 1);
            (std::cmp::Reverse(depth), uri, ancestor)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.as_str().cmp(right.1.as_str()))
    });
    candidates
        .into_iter()
        .next()
        .map(|(_, uri, ancestor)| (uri, ancestor))
}

fn prepare_insertion(
    snapshot: &IndexSnapshot,
    key: &CanonicalKey,
    locale: &str,
    separator: &str,
    value: &str,
) -> Option<PreparedEdit> {
    let (uri, ancestor) = insertion_target(snapshot, key, locale, separator)?;
    let text = snapshot.text(&uri)?;
    let (object_start, object_end, ancestor_key) = match &ancestor {
        Some(entry) => (
            entry.value_range.0.start,
            entry.value_range.0.end,
            Some(&entry.key),
        ),
        None => {
            let start = text.find('{')?;
            let end = text.rfind('}')? + 1;
            (start, end, None)
        }
    };
    let relative = ancestor_key
        .and_then(|ancestor| key.relative_to(ancestor, separator))
        .unwrap_or(key.as_str());
    let segments = relative.split(separator).collect::<Vec<_>>();
    if segments.is_empty() {
        return None;
    }
    let closing = object_end.checked_sub(1)?;
    let inner = text.get(object_start + 1..closing)?;
    let closing_indent = line_indent(text, closing);
    let indent_unit = detect_indent(text).unwrap_or("  ");
    let child_indent = format!("{closing_indent}{indent_unit}");
    let multiline = inner.contains(['\n', '\r']);
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let property = nested_property(
        &segments,
        value,
        &child_indent,
        indent_unit,
        newline,
        multiline,
    )?;
    let (offset, new_text) = if inner.trim().is_empty() {
        if multiline {
            (
                object_start + 1,
                format!("{newline}{property}{newline}{closing_indent}"),
            )
        } else {
            (closing, property.trim_start().to_owned())
        }
    } else if multiline {
        let last = inner.rfind(|character: char| !character.is_whitespace())?;
        (object_start + 1 + last + 1, format!(",{newline}{property}"))
    } else {
        (closing, format!(",{}", property.trim_start()))
    };
    let index = LineIndex::new(text);
    let (line, character) = index.position(text, offset)?;
    Some(PreparedEdit {
        uri: uri.clone(),
        version: snapshot.version(&uri),
        range: Range::new(
            Position::new(line, character),
            Position::new(line, character),
        ),
        new_text,
    })
}

fn line_indent(text: &str, offset: usize) -> &str {
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    text[line_start..offset]
        .split_at(
            text[line_start..offset]
                .find(|character: char| !character.is_whitespace())
                .unwrap_or(offset - line_start),
        )
        .0
}

fn detect_indent(text: &str) -> Option<&str> {
    text.lines()
        .filter_map(|line| {
            let count = line
                .chars()
                .take_while(|character| character.is_whitespace())
                .count();
            (count > 0 && count < line.len()).then_some(&line[..count])
        })
        .min_by_key(|indent| indent.len())
}

fn nested_property(
    segments: &[&str],
    value: &str,
    indent: &str,
    indent_unit: &str,
    newline: &str,
    multiline: bool,
) -> Option<String> {
    let name = serde_json::to_string(segments.first()?).ok()?;
    if segments.len() == 1 {
        return Some(format!(
            "{indent}{name}: {}",
            serde_json::to_string(value).ok()?
        ));
    }
    let next_indent = format!("{indent}{indent_unit}");
    let child = nested_property(
        &segments[1..],
        value,
        &next_indent,
        indent_unit,
        newline,
        multiline,
    )?;
    if multiline {
        Some(format!(
            "{indent}{name}: {{{newline}{child}{newline}{indent}}}"
        ))
    } else {
        Some(format!("{indent}{name}: {{{}}}", child.trim_start()))
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
    let mut ignored_seen = HashSet::new();
    for occurrence in snapshot.ignored_occurrences().filter(|occurrence| {
        !occurrence.range.0.is_empty()
            && (occurrence.kind == OccurrenceKind::ScopeDeclaration
                || occurrence
                    .scope
                    .as_ref()
                    .is_none_or(|scope| !workspace.is_ignored_key(scope)))
    }) {
        let identity = (
            occurrence.uri.clone(),
            occurrence.range.0.start,
            occurrence.range.0.end,
            occurrence.key.clone(),
        );
        if !ignored_seen.insert(identity) {
            continue;
        }
        if let Some(range) =
            location(&snapshot, &occurrence.uri, &occurrence.range).map(|l| l.range)
        {
            by_uri
                .entry(occurrence.uri.clone())
                .or_default()
                .push(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: None,
                    code_description: None,
                    source: Some("locale-breeze".into()),
                    message: format!(
                        "Translation key \"{}\" belongs to an ignored scope",
                        occurrence.key
                    ),
                    related_information: None,
                    tags: None,
                    data: None,
                });
        }
    }
    for entry in snapshot.default_locale_leaf_entries(&workspace.config().default_locale) {
        if workspace.config().unused_keys
            && !snapshot
                .is_leaf_key_used_with_separator(&entry.key, &workspace.config().key_separator)
            && let Some(range) = location(&snapshot, &entry.uri, &entry.key_range).map(|l| l.range)
        {
            by_uri
                .entry(entry.uri.clone())
                .or_default()
                .push(Diagnostic {
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

        std::fs::write(
            temp.path().join("translation.en.json"),
            "{\n\n  \"my_key\": \"Value\"\n}",
        )
        .unwrap();
        workspace.refresh_disk_path(&temp.path().join("translation.en.json"));
        let refreshed = diagnostic_notifications(&workspace, &published);
        assert_eq!(refreshed.len(), 1);
        let refreshed_params: PublishDiagnosticsParams =
            serde_json::from_value(refreshed[0].params.clone()).unwrap();
        assert_eq!(refreshed_params.diagnostics.len(), 1);
        assert_eq!(refreshed_params.diagnostics[0].range.start.line, 2);
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
            unused_keys_override: None,
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
            unused_keys_override: None,
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
        assert_eq!(result, Some(Value::String("Page/Login/submit".into())));
    }

    #[test]
    fn unused_key_override_takes_precedence_only_when_supplied() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("locale-breeze.json");
        std::fs::write(
            &config_path,
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"],
              "unusedKeys":false
            }"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("translation.en.json"),
            r#"{"unused":"Value"}"#,
        )
        .unwrap();
        let configured = WorkspaceIndex::load(temp.path().to_owned(), &config_path).unwrap();
        assert!(!configured.config().unused_keys);
        let overridden = WorkspaceIndex::load_with_unused_override(
            temp.path().to_owned(),
            &config_path,
            Some(true),
        )
        .unwrap();
        assert!(overridden.config().unused_keys);
    }

    #[test]
    fn prepares_a_format_preserving_nested_translation_edit() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"]
            }"#,
        )
        .unwrap();
        let dictionary_path = temp.path().join("translation.en.json");
        std::fs::write(
            &dictionary_path,
            "{\r\n  \"Page\": {\r\n    \"old\": \"Old\"\r\n  }\r\n}\r\n",
        )
        .unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let snapshot = workspace.snapshot();
        let key = CanonicalKey::new("Page.Card.title", ".").unwrap();
        let edit = prepare_insertion(&snapshot, &key, "en", ".", "Hello").unwrap();
        assert_eq!(edit.uri, Url::from_file_path(dictionary_path).unwrap());
        assert_eq!(
            edit.new_text,
            ",\r\n    \"Card\": {\r\n      \"title\": \"Hello\"\r\n    }"
        );
    }

    #[test]
    fn refuses_to_replace_a_leaf_parent() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"]
            }"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("translation.en.json"),
            r#"{"Page":"Blocked"}"#,
        )
        .unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let key = CanonicalKey::new("Page.title", ".").unwrap();
        assert!(prepare_insertion(&workspace.snapshot(), &key, "en", ".", "Title").is_none());
    }

    #[test]
    fn scoped_interpolation_resolves_the_static_relative_scope() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"]
            }"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("translation.en.json"),
            r#"{"Page":{"Steps":{"FieldNames":{"action":"Action"}}}}"#,
        )
        .unwrap();
        let source_path = temp.path().join("StepsComparer.tsx");
        let source = "const i18n=useScopedTranslation('Page.Steps'); i18n.t(`FieldNames.${key}`)";
        std::fs::write(&source_path, source).unwrap();
        let workspace = WorkspaceIndex::load(
            temp.path().to_owned(),
            &temp.path().join("locale-breeze.json"),
        )
        .unwrap();
        let snapshot = workspace.snapshot();
        let uri = Url::from_file_path(source_path).unwrap();
        let character = source.find("FieldNames").unwrap() as u32 + 2;
        let resolved = key_at_position(&snapshot, &uri, Position::new(0, character), ".").unwrap();
        assert_eq!(resolved.as_str(), "Page.Steps.FieldNames");
    }

    #[test]
    fn ignored_scope_usages_warn_without_entering_normal_features() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":["useScopedTranslation"],
              "translationMethods":["t"],
              "fullKeyFunctions":["i18next.t"],
              "unusedKeys":false,
              "ignoredScopes":["Server_Errors"]
            }"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("translation.en.json"),
            r#"{"Page":{"unused":"Unused"},"Server_Errors":{"Invalid":"Invalid"}}"#,
        )
        .unwrap();
        let source_path = temp.path().join("app.ts");
        let source = concat!(
            "i18next.t('Server_Errors.Invalid');\n",
            "const i18n=useScopedTranslation('Server_Errors');\n",
            "i18n.t('Invalid');"
        );
        std::fs::write(&source_path, source).unwrap();
        let workspace = Arc::new(
            WorkspaceIndex::load(
                temp.path().to_owned(),
                &temp.path().join("locale-breeze.json"),
            )
            .unwrap(),
        );
        let published = Mutex::new(HashMap::new());
        let notifications = diagnostic_notifications(&workspace, &published);
        assert_eq!(notifications.len(), 1);
        let params: PublishDiagnosticsParams =
            serde_json::from_value(notifications[0].params.clone()).unwrap();
        assert_eq!(params.diagnostics.len(), 2);
        assert!(params.diagnostics.iter().all(|diagnostic| {
            diagnostic.message.contains("belongs to an ignored scope")
                && diagnostic.severity == Some(DiagnosticSeverity::WARNING)
        }));

        let uri = Url::from_file_path(source_path).unwrap();
        let server = Server {
            workspaces: vec![workspace],
            watchers: vec![],
            config_override: None,
            unused_keys_override: None,
            published_diagnostics: Default::default(),
        };
        let position = Position::new(0, source.find("Server_Errors").unwrap() as u32 + 2);
        let text_position =
            TextDocumentPositionParams::new(TextDocumentIdentifier::new(uri.clone()), position);
        assert!(
            server
                .definition(GotoDefinitionParams {
                    text_document_position_params: text_position.clone(),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                })
                .unwrap()
                .is_none()
        );
        assert!(
            server
                .hover(HoverParams {
                    text_document_position_params: text_position,
                    work_done_progress_params: Default::default(),
                })
                .unwrap()
                .is_none()
        );
    }
}
