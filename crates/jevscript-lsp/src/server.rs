//! The protocol loop.
//!
//! One thread reads messages in order and answers each request before the
//! next, which matches the rest of the workspace: nothing here is async, and
//! the only threads are `lsp-server`'s stdio reader and writer. Text changes
//! apply as they arrive, so a request always sees the buffer as the client
//! last sent it. Diagnostics are published once the queue is empty, so a burst
//! of keystrokes compiles once rather than once per keystroke; an edit to a
//! library republishes every open buffer that imports it.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;
use std::str::FromStr;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, FoldingRangeRequest, GotoDefinition, HoverRequest,
    References, Request as _, SemanticTokensFullRequest,
};
use lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbolParams,
    DocumentSymbolResponse, FoldingRangeParams, GotoDefinitionParams, GotoDefinitionResponse,
    HoverParams, HoverProviderCapability, InitializeParams, OneOf, PublishDiagnosticsParams,
    ReferenceParams, SaveOptions, SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Uri, WorkDoneProgressOptions,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use jevscript_compiler::Resolver;

use crate::diagnostics::{self, DEFAULT_ERROR_REFERENCE};
use crate::line_index::Encoding;
use crate::world::{World, path_from_uri};
use crate::{completion, navigate, outline, semantic};

/// What a boxed error from the loop looks like.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// `initializationOptions` the server understands. Both are optional.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Module search roots for `use` paths that are not relative, tried
    /// before `JEVSCRIPT_PATH` (spec section 3.9). Relative entries resolve
    /// against the first workspace folder.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    /// Where `docs/error-reference.md` is published; each diagnostic links to
    /// `<errorReference>#<code>`.
    #[serde(default)]
    pub error_reference: Option<String>,
}

/// The capabilities the server announces.
pub fn capabilities(encoding: Encoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(encoding.kind()),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                will_save: None,
                will_save_wait_until: None,
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(lsp_types::FoldingRangeProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".to_string()]),
            ..CompletionOptions::default()
        }),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                work_done_progress_options: WorkDoneProgressOptions::default(),
                legend: semantic::legend(),
                range: None,
                full: Some(SemanticTokensFullOptions::Bool(true)),
            },
        )),
        ..ServerCapabilities::default()
    }
}

/// Serve over stdin and stdout until the client says `exit`.
///
/// # Errors
///
/// A protocol error, or an I/O error on the stdio threads.
pub fn run_stdio() -> Result<(), BoxError> {
    let (connection, io) = Connection::stdio();
    run(connection)?;
    io.join()?;
    Ok(())
}

/// Serve one session on `connection`: `initialize`, then requests until
/// `shutdown` and `exit`.
///
/// # Errors
///
/// A protocol error, such as a message before `initialize`.
pub fn run(connection: Connection) -> Result<(), BoxError> {
    let (id, params) = connection.initialize_start()?;
    let init: InitializeParams = serde_json::from_value(params)?;
    let encoding = Encoding::negotiate(
        init.capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_deref()),
    );
    #[allow(deprecated, reason = "older clients send only `rootUri`")]
    let mut roots: Vec<PathBuf> = match &init.workspace_folders {
        Some(folders) => folders
            .iter()
            .filter_map(|f| path_from_uri(&f.uri))
            .collect(),
        None => init
            .root_uri
            .as_ref()
            .and_then(path_from_uri)
            .into_iter()
            .collect(),
    };
    roots.dedup();
    let options: Options = init
        .initialization_options
        .clone()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let mut resolver = Resolver::from_env();
    let base = roots.first().cloned().unwrap_or_default();
    let configured: Vec<PathBuf> = options
        .paths
        .iter()
        .map(|p| {
            if p.is_absolute() {
                p.clone()
            } else {
                base.join(p)
            }
        })
        .collect();
    resolver.roots.splice(0..0, configured);

    let result = serde_json::json!({
        "capabilities": capabilities(encoding),
        "serverInfo": ServerInfo {
            name: "jevscript".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
    });
    connection.initialize_finish(id, result)?;

    let mut server = Server {
        connection: &connection,
        world: World::new(encoding, roots, resolver),
        reference: options
            .error_reference
            .unwrap_or_else(|| DEFAULT_ERROR_REFERENCE.to_string()),
        dirty: BTreeSet::new(),
        touched: BTreeSet::new(),
    };
    server.main_loop()
}

struct Server<'a> {
    connection: &'a Connection,
    world: World,
    reference: String,
    /// Open buffers whose diagnostics are out of date.
    dirty: BTreeSet<String>,
    /// Open buffers whose text changed since the last publish: every open
    /// buffer that imports one is out of date too.
    touched: BTreeSet<String>,
}

enum Flow {
    Continue,
    Exit,
}

impl Server<'_> {
    fn main_loop(&mut self) -> Result<(), BoxError> {
        while let Ok(message) = self.connection.receiver.recv() {
            if let Flow::Exit = self.handle(message)? {
                return Ok(());
            }
            // Take everything already queued before compiling anything.
            while let Ok(message) = self.connection.receiver.try_recv() {
                if let Flow::Exit = self.handle(message)? {
                    return Ok(());
                }
            }
            self.publish_dirty()?;
        }
        Ok(())
    }

    fn handle(&mut self, message: Message) -> Result<Flow, BoxError> {
        match message {
            Message::Request(request) => {
                if self.connection.handle_shutdown(&request)? {
                    return Ok(Flow::Exit);
                }
                let response = self.request(request);
                self.connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(notification) => {
                if notification.method == "exit" {
                    return Ok(Flow::Exit);
                }
                self.notification(notification);
            }
            Message::Response(_) => {}
        }
        Ok(Flow::Continue)
    }

    fn publish_dirty(&mut self) -> Result<(), BoxError> {
        // An importer is linked against its libraries' open buffers, so an
        // edit to a library changes the importer's diagnostics.
        for uri in std::mem::take(&mut self.touched) {
            if let Ok(uri) = Uri::from_str(&uri) {
                for dependent in self.world.dependents(&uri) {
                    self.dirty.insert(dependent.as_str().to_string());
                }
            }
        }
        for uri in std::mem::take(&mut self.dirty) {
            let Ok(uri) = Uri::from_str(&uri) else {
                continue;
            };
            let Some(file) = self.world.file(&uri) else {
                continue;
            };
            let found = diagnostics::diagnostics(&mut self.world, &file, &self.reference);
            let params = PublishDiagnosticsParams {
                uri: uri.clone(),
                diagnostics: found,
                version: self.world.version(&uri),
            };
            self.connection
                .sender
                .send(Message::Notification(Notification::new(
                    PublishDiagnostics::METHOD.to_string(),
                    params,
                )))?;
        }
        Ok(())
    }

    fn notification(&mut self, notification: Notification) {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Some(params) = parse::<DidOpenTextDocumentParams>(notification.params) {
                    let document = params.text_document;
                    self.dirty.insert(document.uri.as_str().to_string());
                    self.touched.insert(document.uri.as_str().to_string());
                    self.world
                        .open(document.uri, document.text, document.version);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Some(params) = parse::<DidChangeTextDocumentParams>(notification.params) {
                    let document = params.text_document;
                    self.world
                        .change(&document.uri, params.content_changes, document.version);
                    self.dirty.insert(document.uri.as_str().to_string());
                    self.touched.insert(document.uri.as_str().to_string());
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Some(params) = parse::<DidCloseTextDocumentParams>(notification.params) {
                    let uri = params.text_document.uri;
                    // Its importers go back to the saved file.
                    for dependent in self.world.dependents(&uri) {
                        self.dirty.insert(dependent.as_str().to_string());
                    }
                    self.world.close(&uri);
                    self.dirty.remove(uri.as_str());
                    self.touched.remove(uri.as_str());
                    let _ = self
                        .connection
                        .sender
                        .send(Message::Notification(Notification::new(
                            PublishDiagnostics::METHOD.to_string(),
                            PublishDiagnosticsParams {
                                uri,
                                diagnostics: Vec::new(),
                                version: None,
                            },
                        )));
                }
            }
            // A saved or changed file on disk can change what every open
            // buffer that imports it compiles to.
            DidSaveTextDocument::METHOD | DidChangeWatchedFiles::METHOD => {
                self.world.invalidate_disk();
                for uri in self.world.open_uris() {
                    self.dirty.insert(uri.as_str().to_string());
                }
            }
            _ => {}
        }
    }

    fn request(&mut self, request: Request) -> Response {
        let id = request.id.clone();
        let result = match request.method.as_str() {
            HoverRequest::METHOD => self.with::<HoverParams, _, _>(request, |s, p| {
                let at = p.text_document_position_params;
                let (file, offset) = s.locate(&at.text_document.uri, at.position)?;
                navigate::hover(&mut s.world, &file, offset)
            }),
            GotoDefinition::METHOD => self.with::<GotoDefinitionParams, _, _>(request, |s, p| {
                let at = p.text_document_position_params;
                let (file, offset) = s.locate(&at.text_document.uri, at.position)?;
                let found = navigate::definition(&mut s.world, &file, offset);
                Some(GotoDefinitionResponse::Array(found))
            }),
            References::METHOD => self.with::<ReferenceParams, _, _>(request, |s, p| {
                let at = p.text_document_position;
                let (file, offset) = s.locate(&at.text_document.uri, at.position)?;
                Some(navigate::references(
                    &mut s.world,
                    &file,
                    offset,
                    p.context.include_declaration,
                ))
            }),
            DocumentSymbolRequest::METHOD => {
                self.with::<DocumentSymbolParams, _, _>(request, |s, p| {
                    let file = s.world.file(&p.text_document.uri)?;
                    let index = file.index.as_ref()?;
                    Some(DocumentSymbolResponse::Nested(outline::document_symbols(
                        index,
                        &file.text,
                        &file.lines,
                    )))
                })
            }
            FoldingRangeRequest::METHOD => {
                self.with::<FoldingRangeParams, _, _>(request, |s, p| {
                    let file = s.world.file(&p.text_document.uri)?;
                    Some(outline::folding_ranges(&file.recovered.lexed))
                })
            }
            Completion::METHOD => self.with::<CompletionParams, _, _>(request, |s, p| {
                let at = p.text_document_position;
                let (file, offset) = s.locate(&at.text_document.uri, at.position)?;
                Some(CompletionResponse::Array(completion::complete(
                    &mut s.world,
                    &file,
                    offset,
                )))
            }),
            SemanticTokensFullRequest::METHOD => {
                self.with::<SemanticTokensParams, _, _>(request, |s, p| {
                    let file = s.world.file(&p.text_document.uri)?;
                    let classified = semantic::classify(&file.text, file.index.as_ref());
                    let data = semantic::encode(&file.text, &file.lines, &classified);
                    Some(SemanticTokensResult::Tokens(SemanticTokens {
                        result_id: None,
                        data,
                    }))
                })
            }
            _ => Err(Response::new_err(
                id.clone(),
                ErrorCode::MethodNotFound as i32,
                format!("unhandled method `{}`", request.method),
            )),
        };
        result.unwrap_or_else(|error| error)
    }

    /// Decode a request's parameters, run `f`, and encode what it returns;
    /// `None` is the protocol's `null`.
    fn with<P, R, F>(&mut self, request: Request, f: F) -> Result<Response, Response>
    where
        P: DeserializeOwned,
        R: serde::Serialize,
        F: FnOnce(&mut Self, P) -> Option<R>,
    {
        let id: RequestId = request.id;
        let params: P = serde_json::from_value(request.params).map_err(|error| {
            Response::new_err(
                id.clone(),
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )
        })?;
        let result = f(self, params);
        Ok(Response::new_ok(id, result))
    }

    fn locate(
        &mut self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<(std::rc::Rc<crate::world::SourceFile>, usize)> {
        let file = self.world.file(uri)?;
        let offset = file.lines.offset(&file.text, position);
        Some((file, offset))
    }
}

fn parse<P: DeserializeOwned>(params: serde_json::Value) -> Option<P> {
    serde_json::from_value(params).ok()
}
