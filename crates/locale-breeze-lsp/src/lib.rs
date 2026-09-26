use anyhow::Result;
use locale_breeze_core::{
    ByteRange, CanonicalKey, ConfigError, DictionaryIssue, EntryKind, IndexSnapshot, LineIndex,
    OccurrenceKind, QualifiedKey, WorkspaceIndex, WorkspacePreferences,
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

pub fn run_stdio(config_override: Option<PathBuf>, show_unused_keys: bool) -> Result<()> {
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
    let mut server = Server::new(config_override, show_unused_keys);
    server.initialize(&connection, &params);
    server.event_loop(&connection)?;
    io_threads.join()?;
    Ok(())
}

struct Server {
    workspaces: Vec<Arc<WorkspaceIndex>>,
    workspace_roots: HashSet<PathBuf>,
    watchers: Vec<RecommendedWatcher>,
    config_override: Option<PathBuf>,
    preferences: WorkspacePreferences,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceIssue {
    version: u8,
    active: bool,
    code: &'static str,
    summary: String,
    remediation: String,
    path: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceStatus {
    version: u8,
    workspace_root: String,
    config_path: String,
    default_locale: String,
    default_dictionary_path: Option<String>,
    dictionary_root_path: String,
    dictionary_file_count: usize,
    total_key_count: usize,
    unused_key_count: usize,
    generation: u64,
}

impl Server {
    fn new(config_override: Option<PathBuf>, show_unused_keys: bool) -> Self {
        Self {
            workspaces: vec![],
            workspace_roots: HashSet::new(),
            watchers: vec![],
            config_override,
            preferences: WorkspacePreferences { show_unused_keys },
            published_diagnostics: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[allow(deprecated)]
    fn initialize(&mut self, connection: &Connection, params: &InitializeParams) {
        let roots: Vec<PathBuf> = params
            .workspace_folders
            .as_ref()
            .map(|folders| {
                folders
                    .iter()
                    .filter_map(|folder| folder.uri.to_file_path().ok())
                    .collect()
            })
            .or_else(|| {
                params
                    .root_uri
                    .as_ref()
                    .and_then(|uri| uri.to_file_path().ok())
                    .map(|root| vec![root])
            })
            .unwrap_or_default();
        let roots = initialization_roots(roots, self.config_override.as_deref());
        for root in roots {
            if let Ok(uri) = Url::from_directory_path(root) {
                self.add_workspace(connection, uri);
            }
        }
    }

    fn add_workspace(&mut self, connection: &Connection, uri: Url) {
        let Ok(root) = uri.to_file_path() else { return };
        self.workspace_roots.insert(root.clone());
        if self.workspaces.iter().any(|w| same_path(w.root(), &root)) {
            return;
        }
        let config_path = self
            .config_override
            .clone()
            .unwrap_or_else(|| root.join("locale-breeze.json"));
        match WorkspaceIndex::load_with_preferences(root.clone(), &config_path, self.preferences) {
            Ok(workspace) => {
                publish_workspace_issue(connection, WorkspaceIssue::clear());
                clear_uri_diagnostic(connection, &config_path, &self.published_diagnostics);
                let workspace = Arc::new(workspace);
                let watched = workspace.clone();
                let watched_config_path = config_path.clone();
                let sender = connection.sender.clone();
                let published_diagnostics = self.published_diagnostics.clone();
                let mut watcher_registered = false;
                match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if let Ok(event) = event {
                        for path in event.paths {
                            let is_dictionary = watched.is_dictionary_path(&path);
                            if let Some(issue) = watched.refresh_disk_path(&path) {
                                let _ = sender.send(Message::Notification(Notification::new(
                                    "localeBreeze/workspaceIssue".into(),
                                    WorkspaceIssue::from_dictionary_issue(issue),
                                )));
                            } else if is_dictionary {
                                let _ = sender.send(Message::Notification(Notification::new(
                                    "localeBreeze/workspaceIssue".into(),
                                    WorkspaceIssue::clear_dictionary(&path),
                                )));
                            }
                        }
                        for notification in
                            diagnostic_notifications(&watched, &published_diagnostics)
                        {
                            if let Some(trace) = diagnostic_trace_notification(&notification) {
                                let _ = sender.send(Message::Notification(trace));
                            }
                            let _ = sender.send(Message::Notification(notification));
                        }
                        let _ = sender.send(Message::Notification(Notification::new(
                            "localeBreeze/workspaceStatus".into(),
                            WorkspaceStatus::from_workspace(&watched, &watched_config_path),
                        )));
                    }
                }) {
                    Ok(mut watcher) => {
                        let workspace_watched =
                            watcher.watch(&root, RecursiveMode::Recursive).is_ok();
                        let dictionary_root = workspace.dictionary_root();
                        let dictionary_watched = dictionary_root.starts_with(&root)
                            || watcher
                                .watch(&dictionary_root, RecursiveMode::Recursive)
                                .is_ok();
                        if workspace_watched && dictionary_watched {
                            self.watchers.push(watcher);
                            watcher_registered = true;
                        }
                    }
                    Err(error) => log(
                        connection,
                        MessageType::WARNING,
                        format!("LocaleBreeze could not watch {}: {error}", root.display()),
                    ),
                }
                if !watcher_registered {
                    publish_workspace_issue(connection, WorkspaceIssue {
                        version: 1, active: true, code: "watcher_failed",
                        summary: "LocaleBreeze cannot watch workspace files".into(),
                        remediation: "Automatic refresh is unavailable. Check file permissions, then restart the language server from LocaleBreeze settings.".into(),
                        path: Some(root.display().to_string()), line: None, column: None,
                    });
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
                    publish_workspace_status(connection, workspace, &config_path);
                }
            }
            Err(error) => {
                publish_config_diagnostic(
                    connection,
                    &config_path,
                    &error,
                    &self.published_diagnostics,
                );
                publish_workspace_issue(
                    connection,
                    WorkspaceIssue::from_config_error(&error, &config_path),
                );
                log(
                    connection,
                    MessageType::ERROR,
                    format!("LocaleBreeze disabled for {}: {error}", root.display()),
                );
            }
        }
    }

    fn workspace_for_uri(&self, uri: &Url) -> Option<&Arc<WorkspaceIndex>> {
        let path = uri.to_file_path().ok()?;
        self.workspaces
            .iter()
            .find(|workspace| workspace.contains_path(&path))
    }

    fn config_path_for(&self, root: &std::path::Path) -> PathBuf {
        self.config_override
            .clone()
            .unwrap_or_else(|| root.join("locale-breeze.json"))
    }

    fn reload_workspaces(&mut self, connection: &Connection) {
        let roots: Vec<_> = self
            .workspace_roots
            .iter()
            .filter_map(|root| Url::from_file_path(root).ok())
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
                        publish_workspace_status(connection, w, &self.config_path_for(w.root()));
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
                            publish_workspace_status(
                                connection,
                                w,
                                &self.config_path_for(w.root()),
                            );
                        }
                    }
                }
            }
            "textDocument/didClose" => {
                if let Ok(p) = parse::<DidCloseTextDocumentParams>(notification.params) {
                    if let Some(w) = self.workspace_for_uri(&p.text_document.uri) {
                        w.close_document(&p.text_document.uri);
                        publish_diagnostics(connection, w, &self.published_diagnostics);
                        publish_workspace_status(connection, w, &self.config_path_for(w.root()));
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
                    publish_workspace_status(connection, w, &self.config_path_for(w.root()));
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
                            self.workspace_roots.retain(|root| !same_path(root, &path));
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
        if snapshot
            .occurrence_at(&uri, offset)
            .is_some_and(|occurrence| occurrence.kind == OccurrenceKind::NamespaceDeclaration)
        {
            let locations = snapshot
                .dictionary_entries_all()
                .filter(|entry| {
                    entry.namespace.as_deref() == key.namespace.as_deref()
                        && entry.locale == workspace.config().default_locale
                })
                .take(1)
                .filter_map(|entry| location(&snapshot, &entry.uri, &entry.key_range))
                .collect::<Vec<_>>();
            return Ok((!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations)));
        }
        let locations = if snapshot.dictionary_at(&uri, offset).is_some() {
            let is_scope = snapshot
                .dictionary_entries(key.namespace.as_deref(), &key.key)
                .iter()
                .any(|entry| entry.kind == EntryKind::Object);
            let occurrences: Vec<_> = if is_scope {
                snapshot.scope_occurrences(
                    key.namespace.as_deref(),
                    &key.key,
                    &workspace.config().key_separator,
                    32,
                )
            } else {
                snapshot
                    .occurrences(key.namespace.as_deref(), &key.key)
                    .iter()
                    .chain(snapshot.dynamic_scope_occurrences(
                        key.namespace.as_deref(),
                        &key.key,
                        &workspace.config().key_separator,
                    ))
                    .collect()
            };
            occurrences
                .into_iter()
                .filter_map(|occurrence| location(&snapshot, &occurrence.uri, &occurrence.range))
                .collect::<Vec<_>>()
        } else {
            snapshot
                .dictionary_entries(key.namespace.as_deref(), &key.key)
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
            .dictionary_entries(key.namespace.as_deref(), &key.key)
            .iter()
            .find(|entry| entry.locale == *default_locale);
        let text = match default_entry {
            Some(entry) if entry.kind == EntryKind::Leaf => entry.value.clone().unwrap_or_default(),
            Some(entry) if entry.kind == EntryKind::Object => {
                let mut children = snapshot
                    .direct_dictionary_children(
                        &key.key,
                        key.namespace.as_deref(),
                        &workspace.config().key_separator,
                    )
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
                            .relative_to(&key.key, &workspace.config().key_separator)
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
            _ => format!("Translation key `{}` does not exist.", key.key.as_str()),
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
            .dictionary_entries(key.namespace.as_deref(), &key.key)
            .iter()
            .any(|e| e.kind == EntryKind::Object)
            || snapshot
                .occurrence_at(
                    &uri,
                    position_offset(&snapshot, &uri, position).unwrap_or(usize::MAX),
                )
                .is_some_and(|o| o.kind == OccurrenceKind::ScopeDeclaration);
        let occurrences: Vec<_> = if is_scope {
            snapshot.scope_occurrences(
                key.namespace.as_deref(),
                &key.key,
                &workspace.config().key_separator,
                32,
            )
        } else {
            snapshot
                .occurrences(key.namespace.as_deref(), &key.key)
                .iter()
                .chain(snapshot.dynamic_scope_occurrences(
                    key.namespace.as_deref(),
                    &key.key,
                    &workspace.config().key_separator,
                ))
                .collect()
        };
        let mut locations: Vec<_> = occurrences
            .into_iter()
            .filter_map(|o| location(&snapshot, &o.uri, &o.range))
            .collect();
        if params.context.include_declaration {
            locations.extend(
                snapshot
                    .dictionary_entries(key.namespace.as_deref(), &key.key)
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
                .map(|key| {
                    Value::String(key.namespace.map_or_else(
                        || key.key.as_str().to_owned(),
                        |namespace| format!("{namespace}:{}", key.key),
                    ))
                }))
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
                            .dictionary_entries(occurrence.namespace.as_deref(), &occurrence.key)
                            .iter()
                            .any(|entry| entry.locale == *default_locale);
                        let can_add = matches!(
                            occurrence.kind,
                            OccurrenceKind::FullKey | OccurrenceKind::ScopedKey
                        ) && !declaration_exists
                            && insertion_target(
                                &snapshot,
                                occurrence.namespace.as_deref(),
                                &occurrence.key,
                                default_locale,
                                &workspace.config().key_separator,
                            )
                            .is_some();
                        Some(DocumentKeyInfo {
                            range,
                            key: occurrence.namespace.as_ref().map_or_else(
                                || occurrence.key.as_str().to_owned(),
                                |namespace| format!("{namespace}:{}", occurrence.key),
                            ),
                            declaration_exists,
                            can_add,
                        })
                    })
                    .collect::<Vec<_>>();
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
                        .dictionary_entries(occurrence.namespace.as_deref(), &occurrence.key)
                        .iter()
                        .any(|entry| entry.locale == workspace.config().default_locale)
                {
                    return Ok(None);
                }
                let Some(edit) = prepare_insertion(
                    &snapshot,
                    occurrence.namespace.as_deref(),
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
    namespace: Option<&str>,
    key: &CanonicalKey,
    locale: &str,
    separator: &str,
) -> Option<(Url, Option<locale_breeze_core::DictionaryEntry>)> {
    let ancestors =
        std::iter::successors(key.parent(separator), |current| current.parent(separator))
            .collect::<Vec<_>>();
    if ancestors.iter().any(|ancestor| {
        snapshot
            .dictionary_entries(namespace, ancestor)
            .iter()
            .any(|entry| entry.locale == locale && entry.kind == EntryKind::Leaf)
    }) {
        return None;
    }
    let mut files = snapshot
        .dictionary_entries_all()
        .filter(|entry| entry.locale == locale && entry.namespace.as_deref() == namespace)
        .map(|entry| entry.uri.clone())
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    files.dedup();
    let mut candidates = files
        .into_iter()
        .map(|uri| {
            let ancestor = ancestors.iter().find_map(|ancestor| {
                snapshot
                    .dictionary_entries(namespace, ancestor)
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
    namespace: Option<&str>,
    key: &CanonicalKey,
    locale: &str,
    separator: &str,
    value: &str,
) -> Option<PreparedEdit> {
    let (uri, ancestor) = insertion_target(snapshot, namespace, key, locale, separator)?;
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
) -> Option<QualifiedKey> {
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
            let key = match scope {
                Some(scope) => CanonicalKey::join(scope, &prefix, separator),
                None => CanonicalKey::new(prefix, separator),
            }?;
            Some(QualifiedKey::new(occurrence.namespace.clone(), key))
        })
        .or_else(|| {
            snapshot
                .dictionary_at(uri, offset)
                .map(|e| QualifiedKey::new(e.namespace.clone(), e.key.clone()))
        })
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

fn initialization_roots(
    roots: Vec<PathBuf>,
    config_override: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    if let Some(config_path) = config_override {
        let preferred = roots
            .iter()
            .position(|root| config_path.starts_with(root))
            .unwrap_or(0);
        return roots.into_iter().nth(preferred).into_iter().collect();
    }

    let configured: Vec<_> = roots
        .iter()
        .filter(|root| root.join("locale-breeze.json").is_file())
        .cloned()
        .collect();
    if configured.is_empty() {
        roots
    } else {
        configured
    }
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
        if workspace.preferences().show_unused_keys
            && !snapshot.is_leaf_entry_used(entry, &workspace.config().key_separator)
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

fn publish_config_diagnostic(
    connection: &Connection,
    config_path: &std::path::Path,
    error: &ConfigError,
    published: &Mutex<HashMap<Url, Vec<Diagnostic>>>,
) {
    let Ok(uri) = Url::from_file_path(config_path) else {
        return;
    };
    let issue = WorkspaceIssue::from_config_error(error, config_path);
    let line = issue.line.unwrap_or(1).saturating_sub(1) as u32;
    let character = issue.column.unwrap_or(1).saturating_sub(1) as u32;
    let diagnostic = Diagnostic {
        range: Range::new(
            Position::new(line, character),
            Position::new(line, character.saturating_add(1)),
        ),
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(issue.code.into())),
        code_description: None,
        source: Some("locale-breeze".into()),
        message: issue.summary,
        related_information: None,
        tags: None,
        data: None,
    };
    let diagnostics = vec![diagnostic];
    let mut cache = published
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.get(&uri) == Some(&diagnostics) {
        return;
    }
    cache.insert(uri.clone(), diagnostics.clone());
    drop(cache);
    let notification = Notification::new(
        "textDocument/publishDiagnostics".into(),
        PublishDiagnosticsParams::new(uri, diagnostics, None),
    );
    let _ = connection.sender.send(Message::Notification(notification));
}

fn clear_uri_diagnostic(
    connection: &Connection,
    path: &std::path::Path,
    published: &Mutex<HashMap<Url, Vec<Diagnostic>>>,
) {
    let Ok(uri) = Url::from_file_path(path) else {
        return;
    };
    let removed = published
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&uri)
        .is_some();
    if removed {
        let notification = Notification::new(
            "textDocument/publishDiagnostics".into(),
            PublishDiagnosticsParams::new(uri, Vec::new(), None),
        );
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

fn publish_workspace_issue(connection: &Connection, issue: WorkspaceIssue) {
    let notification = Notification::new("localeBreeze/workspaceIssue".into(), issue);
    let _ = connection.sender.send(Message::Notification(notification));
}

fn publish_workspace_status(
    connection: &Connection,
    workspace: &WorkspaceIndex,
    config_path: &std::path::Path,
) {
    let notification = Notification::new(
        "localeBreeze/workspaceStatus".into(),
        WorkspaceStatus::from_workspace(workspace, config_path),
    );
    let _ = connection.sender.send(Message::Notification(notification));
}

impl WorkspaceStatus {
    fn from_workspace(workspace: &WorkspaceIndex, config_path: &std::path::Path) -> Self {
        let snapshot = workspace.snapshot();
        let pattern = workspace
            .config()
            .dictionary_pattern()
            .expect("workspace has a validated dictionary pattern");
        let mut dictionary_paths = snapshot
            .files
            .values()
            .filter_map(|file| file.uri.to_file_path().ok())
            .filter_map(|path| {
                pattern
                    .identity_for(workspace.root(), &path)
                    .map(|identity| (identity, path))
            })
            .collect::<Vec<_>>();
        dictionary_paths.sort_by(|left, right| left.1.cmp(&right.1));
        dictionary_paths.dedup_by(|left, right| left.1 == right.1);

        let default_dictionary_path = dictionary_paths
            .iter()
            .find(|(identity, _)| {
                identity.locale == workspace.config().default_locale
                    && identity.namespace == workspace.config().default_namespace
            })
            .map(|(_, path)| path.display().to_string());

        let mut seen_keys = HashSet::new();
        let mut unused_key_count = 0;
        for entry in snapshot.default_locale_leaf_entries(&workspace.config().default_locale) {
            if seen_keys.insert((entry.namespace.clone(), entry.key.as_str().to_owned()))
                && !snapshot.is_leaf_entry_used(entry, &workspace.config().key_separator)
            {
                unused_key_count += 1;
            }
        }

        Self {
            version: 1,
            workspace_root: workspace.root().display().to_string(),
            config_path: config_path.display().to_string(),
            default_locale: workspace.config().default_locale.clone(),
            default_dictionary_path,
            dictionary_root_path: workspace.dictionary_root().display().to_string(),
            dictionary_file_count: dictionary_paths.len(),
            total_key_count: seen_keys.len(),
            unused_key_count,
            generation: snapshot.generation,
        }
    }
}

impl WorkspaceIssue {
    fn clear() -> Self {
        Self {
            version: 1,
            active: false,
            code: "clear",
            summary: String::new(),
            remediation: String::new(),
            path: None,
            line: None,
            column: None,
        }
    }

    fn clear_dictionary(path: &std::path::Path) -> Self {
        Self {
            version: 1,
            active: false,
            code: "dictionary_invalid",
            summary: String::new(),
            remediation: String::new(),
            path: Some(path.display().to_string()),
            line: None,
            column: None,
        }
    }

    fn from_config_error(error: &ConfigError, config_path: &std::path::Path) -> Self {
        let path = Some(config_path.display().to_string());
        let (code, summary, remediation, line, column) = match error {
            ConfigError::Read(_, source) if source.kind() == std::io::ErrorKind::NotFound => (
                "config_missing",
                "LocaleBreeze configuration was not found".into(),
                "Create the configuration file or select the correct file in LocaleBreeze settings.".into(),
                None,
                None,
            ),
            ConfigError::Read(_, _) => (
                "config_unreadable",
                "LocaleBreeze cannot read its configuration".into(),
                "Check that the file exists and that WebStorm has permission to read it.".into(),
                None,
                None,
            ),
            ConfigError::Json(source) => {
                let (code, summary, remediation) = json_config_error_details(source);
                (
                    code,
                    summary,
                    remediation,
                    Some(source.line()),
                    Some(source.column()),
                )
            }
            ConfigError::LocaleToken => (
                "config_dictionary_pattern",
                "The dictionary pattern is invalid".into(),
                "Set `dictionaries` to a relative pattern containing exactly one `{locale}` token, for example `public/dictionaries/translation.{locale}.json`.".into(),
                None,
                None,
            ),
            ConfigError::NamespaceToken => (
                "config_dictionary_pattern",
                "The dictionary namespace pattern is invalid".into(),
                "Use at most one `{namespace}` token in `dictionaries`.".into(),
                None, None,
            ),
            ConfigError::AbsoluteDictionaryPattern => (
                "config_dictionary_pattern",
                "The dictionary pattern must be relative".into(),
                "Use a path relative to the workspace root; parent-directory (`..`) segments are supported.".into(),
                None,
                None,
            ),
            ConfigError::Empty(field) => (
                "config_value",
                format!("LocaleBreeze configuration field `{field}` is empty"),
                "Provide at least one valid value for this field, then save the configuration.".into(),
                None,
                None,
            ),
            ConfigError::InvalidIgnoredScope(scope) => (
                "config_value",
                format!("Ignored scope `{scope}` is not a valid translation key"),
                "Use the configured key separator and remove empty key segments.".into(),
                None,
                None,
            ),
            ConfigError::Glob(_) => (
                "config_dictionary_pattern",
                "The dictionary pattern is not a valid file pattern".into(),
                "Correct the `dictionaries` pattern, then save the configuration.".into(),
                None,
                None,
            ),
            ConfigError::MissingDefaultLocale(locale) => (
                "dictionary_default_missing",
                format!("No dictionary was found for the default locale `{locale}`"),
                "Check `defaultLocale`, the `dictionaries` pattern, and that the matching dictionary file exists and contains valid JSON.".into(),
                None,
                None,
            ),
            ConfigError::MissingDefaultNamespace => (
                "dictionary_default_namespace_missing",
                "LocaleBreeze cannot determine the default namespace".into(),
                "Set `defaultNamespace` when the dictionary pattern matches more than one namespace.".into(),
                None, None,
            ),
            ConfigError::MissingNamespaceDictionary {
                field,
                namespace,
                locale,
            } => (
                "dictionary_namespace_missing",
                format!(
                    "Namespace `{namespace}` configured by `{field}` has no dictionary for locale `{locale}`"
                ),
                "Correct the namespace override or add its default-locale dictionary file.".into(),
                None,
                None,
            ),
            ConfigError::DuplicateFunction { field, name } => (
                "config_value",
                format!("Function `{name}` is configured more than once in `{field}`"),
                "Keep one configuration entry for each function name.".into(),
                None,
                None,
            ),
            ConfigError::InvalidDictionary { path: dictionary, message, line, column } => {
                return Self {
                    version: 1,
                    active: true,
                    code: "dictionary_invalid",
                    summary: "LocaleBreeze could not read a dictionary".into(),
                    remediation: format!("{message}. Correct the dictionary and save it; LocaleBreeze will retry automatically."),
                    path: Some(dictionary.display().to_string()),
                    line: *line,
                    column: *column,
                };
            }
        };
        Self {
            version: 1,
            active: true,
            code,
            summary,
            remediation,
            path,
            line,
            column,
        }
    }

    fn from_dictionary_issue(issue: DictionaryIssue) -> Self {
        Self {
            version: 1,
            active: true,
            code: "dictionary_invalid",
            summary: "LocaleBreeze could not read a dictionary".into(),
            remediation: format!(
                "{}. Correct the dictionary and save it; LocaleBreeze will refresh automatically.",
                issue.message
            ),
            path: Some(issue.path.display().to_string()),
            line: issue.line,
            column: issue.column,
        }
    }
}

fn json_config_error_details(error: &serde_json::Error) -> (&'static str, String, String) {
    if matches!(
        error.classify(),
        serde_json::error::Category::Syntax | serde_json::error::Category::Eof
    ) {
        return (
            "config_json",
            "LocaleBreeze configuration contains invalid JSON".into(),
            "Correct the JSON syntax, then save the file. LocaleBreeze will retry automatically."
                .into(),
        );
    }

    let message = error.to_string();
    let message = message
        .rsplit_once(" at line ")
        .map_or(message.as_str(), |(message, _)| message);
    let summary = if let Some(expected) = typescript_config_expectation(message) {
        format!("Invalid LocaleBreeze configuration value. Expected {expected}")
    } else if let Some(field) = quoted_serde_field(message, "unknown field `") {
        format!("Unknown LocaleBreeze configuration field `{field}`")
    } else if let Some(field) = quoted_serde_field(message, "missing field `") {
        format!("Missing required LocaleBreeze configuration field `{field}`")
    } else if let Some(field) = quoted_serde_field(message, "duplicate field `") {
        format!("Duplicate LocaleBreeze configuration field `{field}`")
    } else {
        format!("Invalid LocaleBreeze configuration: {message}")
    };
    (
        "config_schema",
        summary,
        "Correct the configuration value or field, then save the file. LocaleBreeze will retry automatically."
            .into(),
    )
}

fn typescript_config_expectation(message: &str) -> Option<&'static str> {
    if message.contains("expected struct ScopedFunctionConfig") {
        Some("{ functionName: string; defaultNamespace?: string; translationMethods: string[] }")
    } else if message.contains("expected struct FullKeyFunctionConfig") {
        Some("{ functionName: string; defaultNamespace?: string }")
    } else {
        None
    }
}

fn quoted_serde_field<'a>(message: &'a str, prefix: &str) -> Option<&'a str> {
    message
        .strip_prefix(prefix)?
        .split_once('`')
        .map(|(field, _)| field)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_configuration_shape_errors_from_invalid_json() {
        let unknown = serde_json::from_str::<locale_breeze_core::Config>(
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":[],
              "fullKeyFunctions":[],
              "unexpected":true
            }"#,
        )
        .unwrap_err();
        let issue = WorkspaceIssue::from_config_error(
            &ConfigError::Json(unknown),
            std::path::Path::new("locale-breeze.json"),
        );
        assert_eq!(issue.code, "config_schema");
        assert_eq!(
            issue.summary,
            "Unknown LocaleBreeze configuration field `unexpected`"
        );

        let wrong_scoped_function = serde_json::from_str::<locale_breeze_core::Config>(
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":["useScopedTranslation"],
              "fullKeyFunctions":[]
            }"#,
        )
        .unwrap_err();
        let issue = WorkspaceIssue::from_config_error(
            &ConfigError::Json(wrong_scoped_function),
            std::path::Path::new("locale-breeze.json"),
        );
        assert_eq!(
            issue.summary,
            "Invalid LocaleBreeze configuration value. Expected { functionName: string; defaultNamespace?: string; translationMethods: string[] }"
        );

        let syntax = serde_json::from_str::<locale_breeze_core::Config>("{").unwrap_err();
        let issue = WorkspaceIssue::from_config_error(
            &ConfigError::Json(syntax),
            std::path::Path::new("locale-breeze.json"),
        );
        assert_eq!(issue.code, "config_json");
        assert_eq!(
            issue.summary,
            "LocaleBreeze configuration contains invalid JSON"
        );
    }

    #[test]
    fn publishes_and_clears_config_file_diagnostics() {
        let (server, client) = Connection::memory();
        let published = Mutex::new(HashMap::new());
        let path = std::env::temp_dir().join("locale-breeze-diagnostic-test.json");
        let source = serde_json::from_str::<locale_breeze_core::Config>(r#"{"unexpected":true}"#)
            .unwrap_err();

        publish_config_diagnostic(&server, &path, &ConfigError::Json(source), &published);
        let Message::Notification(notification) = client.receiver.recv().unwrap() else {
            panic!("expected diagnostic notification");
        };
        let params =
            serde_json::from_value::<PublishDiagnosticsParams>(notification.params).unwrap();
        assert_eq!(params.diagnostics.len(), 1);
        assert_eq!(
            params.diagnostics[0].code,
            Some(NumberOrString::String("config_schema".into()))
        );
        assert_eq!(
            params.diagnostics[0].message,
            "Unknown LocaleBreeze configuration field `unexpected`"
        );

        clear_uri_diagnostic(&server, &path, &published);
        let Message::Notification(notification) = client.receiver.recv().unwrap() else {
            panic!("expected clearing notification");
        };
        let params =
            serde_json::from_value::<PublishDiagnosticsParams>(notification.params).unwrap();
        assert!(params.diagnostics.is_empty());
    }

    #[test]
    fn initialization_ignores_attached_content_roots_without_a_config() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("frontend").join("src");
        let dictionaries = temp.path().join("frontend").join("translations");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&dictionaries).unwrap();
        std::fs::write(project.join("locale-breeze.json"), "{}").unwrap();

        assert_eq!(
            initialization_roots(vec![project.clone(), dictionaries], None),
            vec![project]
        );
    }

    #[test]
    fn workspace_status_reports_metrics_when_unused_diagnostics_are_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("app");
        let dictionaries = temp.path().join("translations");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(&dictionaries).unwrap();
        let config_path = root.join("locale-breeze.json");
        std::fs::write(
            &config_path,
            r#"{
              "dictionaries":"../translations/translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
            }"#,
        )
        .unwrap();
        let default_dictionary = dictionaries.join("translation.en.json");
        std::fs::write(&default_dictionary, r#"{"used":"Used","unused":"Unused"}"#).unwrap();
        std::fs::write(
            dictionaries.join("translation.fr.json"),
            r#"{"used":"Utilisé"}"#,
        )
        .unwrap();
        std::fs::write(root.join("src/app.ts"), r#"i18next.t("used")"#).unwrap();

        let workspace = WorkspaceIndex::load(root, &config_path).unwrap();
        let status = WorkspaceStatus::from_workspace(&workspace, &config_path);

        assert_eq!(status.dictionary_file_count, 2);
        assert_eq!(status.total_key_count, 2);
        assert_eq!(status.unused_key_count, 1);
        assert_eq!(status.default_locale, "en");
        assert_eq!(
            status.dictionary_root_path,
            dictionaries.display().to_string()
        );
        assert_eq!(
            status.default_dictionary_path,
            Some(default_dictionary.display().to_string())
        );
        assert!(status.generation > 0);
    }

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
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
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
        let _ = workspace.refresh_disk_path(&temp.path().join("translation.en.json"));
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
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
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
            workspace_roots: HashSet::new(),
            watchers: vec![],
            config_override: None,
            preferences: WorkspacePreferences::default(),
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
              "dictionaries":"locales/{locale}/{namespace}.json",
              "defaultLocale":"en",
              "defaultNamespace":"common",
              "keySeparator":"/",
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
            }"#,
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("locales/en")).unwrap();
        let dictionary_path = temp.path().join("locales/en/common.json");
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
            workspace_roots: HashSet::new(),
            watchers: vec![],
            config_override: None,
            preferences: WorkspacePreferences::default(),
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
        assert_eq!(
            result,
            Some(Value::String("common:Page/Login/submit".into()))
        );
    }

    #[test]
    fn runtime_preferences_control_unused_key_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("locale-breeze.json");
        std::fs::write(
            &config_path,
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
            }"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("translation.en.json"),
            r#"{"unused":"Value"}"#,
        )
        .unwrap();
        let configured = WorkspaceIndex::load(temp.path().to_owned(), &config_path).unwrap();
        assert!(configured.preferences().show_unused_keys);
        let hidden = WorkspaceIndex::load_with_preferences(
            temp.path().to_owned(),
            &config_path,
            WorkspacePreferences {
                show_unused_keys: false,
            },
        )
        .unwrap();
        assert!(!hidden.preferences().show_unused_keys);
    }

    #[test]
    fn prepares_a_format_preserving_nested_translation_edit() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
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
        let edit = prepare_insertion(&snapshot, None, &key, "en", ".", "Hello").unwrap();
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
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
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
        assert!(prepare_insertion(&workspace.snapshot(), None, &key, "en", ".", "Title").is_none());
    }

    #[test]
    fn scoped_interpolation_resolves_the_static_relative_scope() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}]
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
        assert_eq!(resolved.key.as_str(), "Page.Steps.FieldNames");
    }

    #[test]
    fn ignored_scope_usages_warn_without_entering_normal_features() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("locale-breeze.json"),
            r#"{
              "dictionaries":"translation.{locale}.json",
              "defaultLocale":"en",
              "scopedFunctions":[{"functionName":"useScopedTranslation","translationMethods":["t"]}],
              "fullKeyFunctions":[{"functionName":"translate"}],
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
            WorkspaceIndex::load_with_preferences(
                temp.path().to_owned(),
                &temp.path().join("locale-breeze.json"),
                WorkspacePreferences {
                    show_unused_keys: false,
                },
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
            workspace_roots: HashSet::new(),
            watchers: vec![],
            config_override: None,
            preferences: WorkspacePreferences::default(),
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
