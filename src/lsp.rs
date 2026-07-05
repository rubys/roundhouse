//! Standalone read-only LSP server — Rung 1 of roundhouse#57.
//!
//! Wires the [`crate::ide`] query layer to editors over the Language
//! Server Protocol. Read-only by design: it publishes diagnostics and
//! answers `hover` / `inlayHint` / `completion`, and never proposes
//! edits — so the server is a pure analysis process with no filesystem
//! side effects (it reads the workspace; the editor owns every buffer).
//!
//! Transport is the *synchronous* `lsp-server` crate (rust-analyzer's),
//! deliberately: the analysis engine is sync and fast (~sub-second
//! whole-app), so there is no async runtime here and no incrementality —
//! every pass re-ingests and re-analyses the whole app through an
//! overlay VFS that layers open buffers on top of disk.
//!
//! ## Model
//!
//! The analyzer ingests a whole Rails app, not a file. The server keeps
//! the workspace root, an overlay of open buffers (path → current
//! text), and the **last-good analysis** (typed [`App`] + the class
//! registry) behind a shared slot. The first `didOpen` analyses
//! synchronously so a fresh session has hovers immediately; every
//! subsequent change is snapshotted to a debounced background worker,
//! which re-analyses and swaps the slot when it finishes. The request
//! loop therefore never blocks behind an analysis: queries — most
//! importantly completion, which arrives on the very keystroke that
//! made the buffer dirty — answer from the previous snapshot
//! (stale-by-one-edit, the standard fast-language-server trade), and
//! diagnostics catch up when the worker publishes.

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lsp_server::{Connection, Message, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    Completion, GotoDefinition, HoverRequest, InlayHintRequest, References, Request as _,
};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionOptions,
    CompletionParams, CompletionResponse, Diagnostic as LspDiagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    InitializeParams, InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, Location,
    MarkupContent, MarkupKind, OneOf, Position as LspPosition, PublishDiagnosticsParams,
    Range as LspRange, ReferenceParams, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri,
};

use crate::analyze::{diagnose, Analyzer, ClassInfo, Severity};
use crate::app::App;
use crate::diagnostic::{Diagnostic as RhDiagnostic, DiagnosticKind};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ide;
use crate::ident::{ClassId, Symbol};
use crate::ingest::ingest_app_with_vfs;
use crate::span::Span;
use crate::ty::Ty;
use crate::vfs::{FsVfs, Vfs};

type LspResult<T> = Result<T, Box<dyn Error + Sync + Send>>;

/// Entry point for the `roundhouse-lsp` binary: serve over stdio.
pub fn run() -> LspResult<()> {
    let (connection, io_threads) = Connection::stdio();
    run_connection(connection)?;
    io_threads.join()?;
    Ok(())
}

/// Drive the protocol over an arbitrary connection (stdio in production,
/// `Connection::memory()` in tests). Performs the initialize handshake,
/// then runs the message loop to completion.
pub fn run_connection(connection: Connection) -> LspResult<()> {
    let capabilities = serde_json::to_value(server_capabilities())?;
    let init_params = connection.initialize(capabilities)?;
    let init: InitializeParams = serde_json::from_value(init_params)?;
    let root = workspace_root(&init);
    Server::new(connection, root).main_loop()
}

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        // Full-document sync: at sub-second whole-app analysis there is no
        // need for incremental text edits.
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            // `.` member completion, `@` ivars, `(`/`,`/` ` typed-kwarg
            // completion inside `find_by(`/`where(` argument lists.
            trigger_characters: Some(
                [".", "@", "(", ",", " "].iter().map(|s| s.to_string()).collect(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Best workspace root from the initialize params: first workspace folder,
/// else the (deprecated) root URI, else the process CWD.
fn workspace_root(init: &InitializeParams) -> PathBuf {
    if let Some(folders) = &init.workspace_folders {
        if let Some(first) = folders.first() {
            if let Some(p) = uri_to_path(&first.uri) {
                return p;
            }
        }
    }
    #[allow(deprecated)]
    if let Some(uri) = &init.root_uri {
        if let Some(p) = uri_to_path(uri) {
            return p;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// One completed whole-app analysis: the typed [`App`] plus the
/// analyzer's class registry (the member tables completion enumerates).
/// Handed around behind an `Arc` so query handlers hold a consistent
/// snapshot while the worker swaps in a newer one.
struct Analysis {
    app: App,
    registry: HashMap<ClassId, ClassInfo>,
}

/// The last-good analysis, shared between the request loop (readers)
/// and the background worker (writer). Queries — including completion,
/// which arrives on the very keystroke that triggered a reanalysis —
/// answer from this snapshot immediately instead of queueing behind a
/// 150–500ms whole-app re-ingest; stale-by-one-edit answers are the
/// standard fast-language-server trade.
type SharedAnalysis = Arc<Mutex<Option<Arc<Analysis>>>>;

/// A snapshot of everything the worker needs for one analysis pass.
struct AnalyzeRequest {
    overlay: HashMap<PathBuf, String>,
    open: Vec<Uri>,
}

struct Server {
    connection: Connection,
    root: PathBuf,
    /// Open buffers, keyed by canonical absolute path → current text.
    overlay: HashMap<PathBuf, String>,
    /// Open document URIs, in open order — the set we publish (and clear)
    /// diagnostics for each analysis pass.
    open: Vec<Uri>,
    /// Last analysis that ingested+analysed cleanly; what every query
    /// reads. Written synchronously on the first open (so a fresh editor
    /// session has hovers the moment the file is open) and by the
    /// background worker thereafter.
    analysis: SharedAnalysis,
    /// Channel to the debounced background analysis worker.
    worker: mpsc::Sender<AnalyzeRequest>,
}

impl Server {
    fn new(connection: Connection, root: PathBuf) -> Self {
        let analysis: SharedAnalysis = Arc::new(Mutex::new(None));
        let worker = spawn_analysis_worker(
            root.clone(),
            connection.sender.clone(),
            Arc::clone(&analysis),
        );
        Self { connection, root, overlay: HashMap::new(), open: Vec::new(), analysis, worker }
    }

    /// The current last-good analysis snapshot, if any pass has
    /// completed. Clones the `Arc` out of the lock so the query runs
    /// without blocking the worker's swap.
    fn current(&self) -> Option<Arc<Analysis>> {
        self.analysis.lock().ok()?.clone()
    }

    fn main_loop(&mut self) -> LspResult<()> {
        while let Ok(msg) = self.connection.receiver.recv() {
            match msg {
                Message::Request(req) => {
                    if self.connection.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    self.on_request(req)?;
                }
                Message::Notification(not) => {
                    if not.method == Exit::METHOD {
                        return Ok(());
                    }
                    self.on_notification(not)?;
                }
                Message::Response(_) => {}
            }
        }
        Ok(())
    }

    fn on_request(&mut self, req: Request) -> LspResult<()> {
        if req.method == HoverRequest::METHOD {
            let (id, params) = extract::<HoverParams>(req)?;
            let result = self.hover(params);
            self.respond(id, &result)?;
        } else if req.method == InlayHintRequest::METHOD {
            let (id, params) = extract::<InlayHintParams>(req)?;
            let result = self.inlay_hints(params);
            self.respond(id, &result)?;
        } else if req.method == References::METHOD {
            let (id, params) = extract::<ReferenceParams>(req)?;
            let result = self.references(params);
            self.respond(id, &result)?;
        } else if req.method == GotoDefinition::METHOD {
            let (id, params) = extract::<GotoDefinitionParams>(req)?;
            let result = self.goto_definition(params);
            self.respond(id, &result)?;
        } else if req.method == Completion::METHOD {
            let (id, params) = extract::<CompletionParams>(req)?;
            let result = self.completion(params).map(CompletionResponse::Array);
            self.respond(id, &result)?;
        } else {
            // Unknown request: reply MethodNotFound so the client doesn't
            // block waiting on a method we never advertised.
            let resp = Response {
                id: req.id,
                result: None,
                error: Some(lsp_server::ResponseError {
                    code: -32601, // JSON-RPC MethodNotFound
                    message: format!("unhandled method: {}", req.method),
                    data: None,
                }),
            };
            self.connection.sender.send(Message::Response(resp))?;
        }
        Ok(())
    }

    fn on_notification(&mut self, not: lsp_server::Notification) -> LspResult<()> {
        if not.method == DidOpenTextDocument::METHOD {
            let p: DidOpenTextDocumentParams = serde_json::from_value(not.params)?;
            self.set_buffer(&p.text_document.uri, p.text_document.text);
            if !self.open.iter().any(|u| u == &p.text_document.uri) {
                self.open.push(p.text_document.uri);
            }
            self.schedule_reanalyze()?;
        } else if not.method == DidChangeTextDocument::METHOD {
            let p: DidChangeTextDocumentParams = serde_json::from_value(not.params)?;
            // Full sync: the last change carries the whole document.
            if let Some(change) = p.content_changes.into_iter().next_back() {
                self.set_buffer(&p.text_document.uri, change.text);
            }
            self.schedule_reanalyze()?;
        } else if not.method == DidCloseTextDocument::METHOD {
            let p: DidCloseTextDocumentParams = serde_json::from_value(not.params)?;
            if let Some(path) = uri_to_path(&p.text_document.uri) {
                self.overlay.remove(&canonical(&path));
            }
            self.open.retain(|u| u != &p.text_document.uri);
            // Clear diagnostics for the file we no longer track.
            publish_for(&self.connection.sender, &p.text_document.uri, Vec::new())?;
            self.schedule_reanalyze()?;
        }
        Ok(())
    }

    /// Route a state change into analysis. The first pass (no snapshot
    /// yet) runs synchronously so open→hover works immediately and
    /// deterministically; every subsequent pass goes to the debounced
    /// background worker, keeping this loop free to answer queries —
    /// most importantly completion, which arrives on the very keystroke
    /// that made the buffer dirty.
    fn schedule_reanalyze(&mut self) -> LspResult<()> {
        let request =
            AnalyzeRequest { overlay: self.overlay.clone(), open: self.open.clone() };
        if self.current().is_none() {
            run_and_publish(&self.root, request, &self.connection.sender, &self.analysis);
            return Ok(());
        }
        // A dead worker (panicked analysis) falls back to synchronous —
        // degraded latency beats an editor that stops updating.
        if let Err(mpsc::SendError(request)) = self.worker.send(request) {
            run_and_publish(&self.root, request, &self.connection.sender, &self.analysis);
        }
        Ok(())
    }

    fn set_buffer(&mut self, uri: &Uri, text: String) {
        if let Some(path) = uri_to_path(uri) {
            self.overlay.insert(canonical(&path), text);
        }
    }

    fn hover(&self, params: HoverParams) -> Option<Hover> {
        let analysis = self.current()?;
        let app = &analysis.app;
        let tdp = params.text_document_position_params;
        let path = uri_to_path(&tdp.text_document.uri)?;
        let pos = ide::Position { line: tdp.position.line, character: tdp.position.character };
        let info = ide::type_at_position(app, path.to_str()?, pos)?;

        let mut value = format!("```ruby\n{}\n```", info.display);
        if info.nilable {
            value.push_str("\n\nMay be `nil`.");
        }
        let text = &ide::source(app, info.span.file)?.text;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(span_to_range(text, info.span)),
        })
    }

    fn inlay_hints(&self, params: InlayHintParams) -> Vec<InlayHint> {
        let Some(analysis) = self.current() else { return Vec::new() };
        let app = &analysis.app;
        let Some(path) = uri_to_path(&params.text_document.uri) else { return Vec::new() };
        let Some(path_str) = path.to_str() else { return Vec::new() };
        let Some(file) = ide::file_id(app, path_str) else { return Vec::new() };
        let Some(src) = ide::source(app, file) else { return Vec::new() };
        let text = &src.text;

        let start = ide::position_to_offset(text, lsp_to_ide(params.range.start));
        let end = ide::position_to_offset(text, lsp_to_ide(params.range.end));

        let mut hints = Vec::new();
        ide::nodes_in_range(app, file, start, end, &mut |e| {
            if let Some(hint) = local_assign_hint(e, text) {
                hints.push(hint);
            }
        });
        hints
    }

    fn references(&self, params: ReferenceParams) -> Option<Vec<Location>> {
        let analysis = self.current()?;
        let app = &analysis.app;
        let tdp = params.text_document_position;
        let path = uri_to_path(&tdp.text_document.uri)?;
        let file = ide::file_id(app, path.to_str()?)?;
        let src = ide::source(app, file)?;
        let offset = ide::position_to_offset(&src.text, lsp_to_ide(tdp.position));
        let include_decl = params.context.include_declaration;
        let decl = ide::definition(app, file, offset);
        let locations = ide::references(app, file, offset)
            .into_iter()
            // Only type-certain matches: a flat LSP Location list can't
            // convey confidence, so we keep the precise set (uncertain
            // name-only method matches surface in the MCP, which can label).
            .filter(|r| r.certain)
            .filter(|r| include_decl || Some(r.span) != decl)
            .filter_map(|r| location_of(app, r.span))
            .collect();
        Some(locations)
    }

    fn goto_definition(&self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let analysis = self.current()?;
        let app = &analysis.app;
        let tdp = params.text_document_position_params;
        let path = uri_to_path(&tdp.text_document.uri)?;
        let file = ide::file_id(app, path.to_str()?)?;
        let src = ide::source(app, file)?;
        let offset = ide::position_to_offset(&src.text, lsp_to_ide(tdp.position));
        let span = ide::definition(app, file, offset)?;
        Some(GotoDefinitionResponse::Scalar(location_of(app, span)?))
    }

    // ── Completion ────────────────────────────────────────────────────
    //
    // Answers from the last-good analysis and the *current* buffer: the
    // receiver expression almost always predates the keystroke that
    // triggered the request (`user` existed before its `.` was typed),
    // so typing it against the stale snapshot is correct in practice,
    // and the client filters items against the live prefix as the user
    // keeps typing. Three shapes:
    //   `recv.…`         → members of the receiver's inferred class
    //                      (instance side for values, class side for
    //                      constants — scopes, finders)
    //   `@…`             → ivars observed in this file, with types
    //   `find_by(…`/`where(…` → the receiver model's columns as kwargs

    fn completion(&self, params: CompletionParams) -> Option<Vec<CompletionItem>> {
        let analysis = self.current()?;
        let tdp = params.text_document_position;
        let path = uri_to_path(&tdp.text_document.uri)?;
        let text = self.overlay.get(&canonical(&path))?.as_str();
        let path_str = path.to_str()?;
        let cursor = ide::position_to_offset(text, lsp_to_ide(tdp.position)) as usize;
        let cursor = cursor.min(text.len());

        let start = word_start(text, cursor);
        let word = &text[start..cursor];
        if word.starts_with('@') {
            return self.ivar_items(&analysis.app, path_str);
        }
        let before = text.as_bytes().get(start.checked_sub(1)?)?;
        match before {
            b'.' => self.member_items(&analysis, path_str, text, start - 1),
            b'(' | b',' | b' ' => self.kwarg_items(&analysis, path_str, text, start),
            _ => None,
        }
    }

    /// Members of the receiver ending just before the `.` at `dot`.
    fn member_items(
        &self,
        analysis: &Analysis,
        path: &str,
        text: &str,
        dot: usize,
    ) -> Option<Vec<CompletionItem>> {
        let (class_id, side) = receiver_class(analysis, path, text, dot)?;
        let members = ide::members_of(&analysis.app, &analysis.registry, &class_id, side);
        if members.is_empty() {
            return None;
        }
        Some(members.iter().map(member_item).collect())
    }

    /// Ivars observed anywhere in this file, with their inferred types.
    /// Works in controllers and — because template spans point into the
    /// template file — in ERB/HAML views, where the ivar set *is* the
    /// controller's contribution.
    fn ivar_items(&self, app: &App, path: &str) -> Option<Vec<CompletionItem>> {
        let seen = file_ivars(app, path);
        if seen.is_empty() {
            return None;
        }
        let mut items: Vec<CompletionItem> = seen
            .into_iter()
            .map(|(name, ty)| {
                let display =
                    ty.as_ref().map(ide::render_ty).unwrap_or_else(|| "untyped".to_string());
                CompletionItem {
                    label: format!("@{}", name.as_str()),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(display.clone()),
                    label_details: Some(CompletionItemLabelDetails {
                        detail: None,
                        description: Some(display),
                    }),
                    ..Default::default()
                }
            })
            .collect();
        items.sort_by(|a, b| a.label.cmp(&b.label));
        Some(items)
    }

    /// Column kwargs inside a `find_by(`/`where(` argument list: resolve
    /// the call's receiver to a model class and offer its columns as
    /// `name:` items with their types.
    fn kwarg_items(
        &self,
        analysis: &Analysis,
        path: &str,
        text: &str,
        upto: usize,
    ) -> Option<Vec<CompletionItem>> {
        let dot = kwarg_call_dot(text, upto)?;
        // Whichever side the receiver resolved on (`User.find_by(` is the
        // class object), the kwargs are the model's columns — enumerate
        // the instance side of the resolved class.
        let (class_id, _) = receiver_class(analysis, path, text, dot)?;
        let members = ide::members_of(
            &analysis.app,
            &analysis.registry,
            &class_id,
            ide::MemberSide::Instance,
        );
        let items: Vec<CompletionItem> = members
            .iter()
            .filter(|m| m.kind == ide::MemberKind::Column && !m.name.as_str().ends_with('='))
            .map(|m| CompletionItem {
                label: format!("{}:", m.name.as_str()),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(m.display.clone()),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(m.display.clone()),
                }),
                insert_text: Some(format!("{}: ", m.name.as_str())),
                ..Default::default()
            })
            .collect();
        if items.is_empty() { None } else { Some(items) }
    }

    fn respond<T: serde::Serialize>(&self, id: RequestId, result: &T) -> LspResult<()> {
        let resp = Response { id, result: Some(serde_json::to_value(result)?), error: None };
        self.connection.sender.send(Message::Response(resp))?;
        Ok(())
    }
}

// ── Background analysis worker ───────────────────────────────────────

/// Spawn the debounced analysis worker: it absorbs bursts of
/// `AnalyzeRequest`s while the user types (keeping only the newest),
/// waits for a short quiet period, runs the whole-app pass, swaps the
/// snapshot into `shared`, and publishes diagnostics. The request loop
/// never blocks behind an analysis — completion answers from the
/// previous snapshot while this thread works.
fn spawn_analysis_worker(
    root: PathBuf,
    sender: crossbeam_channel::Sender<Message>,
    shared: SharedAnalysis,
) -> mpsc::Sender<AnalyzeRequest> {
    let (tx, rx) = mpsc::channel::<AnalyzeRequest>();
    std::thread::spawn(move || {
        while let Ok(mut request) = rx.recv() {
            // Debounce: keep absorbing newer snapshots until the channel
            // has been quiet for the window. 150ms is far below a typing
            // pause but enough to coalesce a burst of didChange events.
            loop {
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(newer) => request = newer,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            run_and_publish(&root, request, &sender, &shared);
        }
    });
    tx
}

/// One full analysis pass: ingest through the overlay, analyze, swap
/// the last-good snapshot, publish diagnostics for the open documents.
/// Used by both the worker and the synchronous first-open path. Send
/// failures are ignored — they only mean the client has gone away.
fn run_and_publish(
    root: &Path,
    request: AnalyzeRequest,
    sender: &crossbeam_channel::Sender<Message>,
    shared: &SharedAnalysis,
) {
    let (diags, analysis) = run_analysis(root, &request.overlay);
    if let Some(analysis) = analysis {
        let analysis = Arc::new(analysis);
        if let Ok(mut slot) = shared.lock() {
            *slot = Some(Arc::clone(&analysis));
        }
        publish(sender, &analysis.app, &request.open, diags);
    } else if let Ok(slot) = shared.lock() {
        // Hard ingest failure (rare under Prism error recovery): keep
        // the last good snapshot for queries, surface what we captured.
        if let Some(analysis) = slot.clone() {
            drop(slot);
            publish(sender, &analysis.app, &request.open, diags);
        }
    }
}

fn run_analysis(
    root: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> (Vec<RhDiagnostic>, Option<Analysis>) {
    let vfs = OverlayVfs { disk: FsVfs::new(), overlay };
    // Survey mode: degrade past unsupported constructs (every real app
    // has some) with a placeholder + recorded gap instead of aborting
    // the whole ingest. Without it, one exotic node anywhere leaves the
    // editor inert — no hovers, no diagnostics — on any app past the
    // demo fixture. The gaps themselves aren't published as squiggles
    // (they have no resolvable span), but they drive attribution:
    // diagnostics that are shadows of a gap downgrade to Info and
    // render as hints, not accusations on the user's code.
    crate::ingest::survey::activate();
    let (result, mut parse_diags) =
        crate::ingest::prism::scope(|| ingest_app_with_vfs(&vfs, root));
    let gaps = crate::ingest::survey::drain();
    match result {
        Ok(mut app) => {
            let mut analyzer = Analyzer::new(&app);
            analyzer.analyze(&mut app);
            let registry = analyzer.class_registry().clone();
            let mut diags = diagnose(&app);
            crate::analyze::attribution::attribute_ingest_gaps(&mut diags, &app, &gaps);
            diags.append(&mut parse_diags);
            (diags, Some(Analysis { app, registry }))
        }
        Err(err) => {
            eprintln!("roundhouse-lsp: ingest failed: {err}");
            (parse_diags, None)
        }
    }
}

fn publish(sender: &crossbeam_channel::Sender<Message>, app: &App, open: &[Uri], diags: Vec<RhDiagnostic>) {
    // Group by source path, then convert. Grouping on the roundhouse
    // diagnostics (pre-conversion) lets us apply the mid-edit
    // suppression heuristic by kind.
    let mut by_path: HashMap<PathBuf, Vec<RhDiagnostic>> = HashMap::new();
    for d in diags {
        if let Some(src) = ide::source(app, d.span.file) {
            by_path.entry(canonical(Path::new(&src.path))).or_default().push(d);
        }
    }

    // Mid-keystroke noise control: if a file has a syntax error, the
    // type analysis ran on a half-parsed tree — suppress the derived
    // type diagnostics and show only the syntax errors, so squiggles
    // don't flash on every keystroke.
    for group in by_path.values_mut() {
        if group.iter().any(|d| matches!(d.kind, DiagnosticKind::Parse { .. })) {
            group.retain(|d| matches!(d.kind, DiagnosticKind::Parse { .. }));
        }
    }

    // Publish for every open document — including an empty list to
    // clear diagnostics that a previous pass set but this one cleared.
    for uri in open {
        let items = uri_to_path(uri)
            .and_then(|p| by_path.get(&canonical(&p)))
            .map(|ds| convert_diags(app, ds))
            .unwrap_or_default();
        let _ = publish_for(sender, uri, items);
    }
}

fn convert_diags(app: &App, diags: &[RhDiagnostic]) -> Vec<LspDiagnostic> {
    diags
        .iter()
        .filter_map(|d| {
            let text = &ide::source(app, d.span.file)?.text;
            Some(LspDiagnostic {
                range: span_to_range(text, d.span),
                severity: Some(map_severity(d.severity)),
                source: Some("roundhouse".to_string()),
                message: d.message.clone(),
                ..Default::default()
            })
        })
        .collect()
}

fn publish_for(
    sender: &crossbeam_channel::Sender<Message>,
    uri: &Uri,
    diagnostics: Vec<LspDiagnostic>,
) -> LspResult<()> {
    let params = PublishDiagnosticsParams { uri: uri.clone(), diagnostics, version: None };
    let not = lsp_server::Notification::new(PublishDiagnostics::METHOD.to_string(), params);
    sender.send(Message::Notification(not))?;
    Ok(())
}

/// An LSP `Location` for a span — its file as a `file://` URI plus the
/// UTF-16 range. `None` for the synthetic file or an unencodable path.
fn location_of(app: &App, span: Span) -> Option<Location> {
    let src = ide::source(app, span.file)?;
    Some(Location { uri: path_to_uri(Path::new(&src.path))?, range: span_to_range(&src.text, span) })
}

// ── Completion text scanning ─────────────────────────────────────────

/// Bytes that extend the word being completed: Ruby identifier chars,
/// the ivar sigil, `?`/`!` method suffixes, and `:` so qualified
/// constants (`Admin::Report`) scan as one token.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'@' | b'?' | b'!' | b':')
}

/// Start offset of the word containing/preceding `cursor`.
fn word_start(text: &str, cursor: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = cursor.min(bytes.len());
    while i > 0 && is_word_byte(bytes[i - 1]) {
        i -= 1;
    }
    i
}

/// Resolve the receiver expression ending just before the `.` at `dot`
/// to a class and which side of it the member lookup should use.
///
/// Primary path: ask the last-good analysis for the type at the
/// receiver's final character — the tightest covering node is the
/// receiver itself (a `Var` read, or the full `Send` chain for
/// `a.b.first.`), and its syntactic kind distinguishes the class
/// object (`Const` → class side: scopes, finders) from a value
/// (instance side: columns, associations). Falls back to a textual
/// constant lookup for a class name typed since the last pass.
fn receiver_class(
    analysis: &Analysis,
    path: &str,
    text: &str,
    dot: usize,
) -> Option<(ClassId, ide::MemberSide)> {
    if dot == 0 {
        return None;
    }
    let pos = ide::offset_to_position(text, (dot - 1) as u32);
    if let Some(info) = ide::type_at_position(&analysis.app, path, pos) {
        if let Some(id) = info.ty.as_ref().and_then(root_class) {
            let side = if info.node_kind == "Const" {
                ide::MemberSide::Class
            } else {
                ide::MemberSide::Instance
            };
            return Some((id, side));
        }
    }
    // Textual fallbacks for receivers at positions the stale snapshot
    // can't type (a freshly typed line): a constant receiver resolves
    // by registry name; an ivar receiver resolves by the type the same
    // ivar has elsewhere in this file.
    let start = word_start(text, dot);
    let token = &text[start..dot];
    if token.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        let id = ClassId(Symbol::from(token));
        if analysis.registry.contains_key(&id) {
            return Some((id, ide::MemberSide::Class));
        }
    }
    if let Some(name) = token.strip_prefix('@') {
        let ivars = file_ivars(&analysis.app, path);
        let ty = ivars.get(&Symbol::from(name)).and_then(|t| t.as_ref())?;
        return root_class(ty).map(|id| (id, ide::MemberSide::Instance));
    }
    None
}

/// Every ivar observed in `path`'s analyzed exprs, with the first
/// resolved type seen for each (reads and assignment values both
/// contribute). The substrate for `@` completion and for typing an
/// ivar receiver on a line the stale snapshot hasn't seen.
fn file_ivars(app: &App, path: &str) -> HashMap<Symbol, Option<Ty>> {
    let mut seen: HashMap<Symbol, Option<Ty>> = HashMap::new();
    let Some(file) = ide::file_id(app, path) else { return seen };
    ide::nodes_in_range(app, file, 0, u32::MAX, &mut |e| {
        match &*e.node {
            ExprNode::Ivar { name } => {
                let slot = seen.entry(name.clone()).or_insert(None);
                if slot.is_none() {
                    *slot = e.ty.clone();
                }
            }
            ExprNode::Assign { target: LValue::Ivar { name }, value } => {
                let slot = seen.entry(name.clone()).or_insert(None);
                if slot.is_none() {
                    *slot = value.ty.clone();
                }
            }
            _ => {}
        }
    });
    seen
}

/// The root class of a receiver type: `Article` itself, or the class
/// arm of a nilable union (`Article?` completes as `Article` — the
/// user gets members while the nil-safety diagnostics separately flag
/// the unguarded call).
fn root_class(ty: &Ty) -> Option<ClassId> {
    match ty {
        Ty::Class { id, .. } => Some(id.clone()),
        Ty::Union { variants } => variants.iter().find_map(root_class),
        _ => None,
    }
}

/// For a cursor inside a call's argument list, the offset of the `.`
/// linking the receiver to a kwarg-completable method (`find_by`,
/// `find_by!`, `where`). Scans left for the innermost unbalanced `(`,
/// then requires `recv.method(` immediately before it.
fn kwarg_call_dot(text: &str, upto: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut open = None;
    for i in (0..upto.min(bytes.len())).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' if depth == 0 => {
                open = Some(i);
                break;
            }
            b'(' => depth -= 1,
            _ => {}
        }
    }
    let open = open?;
    let mstart = word_start(text, open);
    let method = &text[mstart..open];
    if !matches!(method, "find_by" | "find_by!" | "where") {
        return None;
    }
    let dot = mstart.checked_sub(1)?;
    (bytes[dot] == b'.').then_some(dot)
}

/// A completion item for one enumerated member. `sort_text` ranks the
/// Rails-shaped members (columns, associations, scopes) above the
/// generic framework/method surface so `user.` leads with `email`,
/// `posts` — the differentiated answers — not `becomes!`.
fn member_item(m: &ide::Member) -> CompletionItem {
    use ide::MemberKind;
    let kind = match m.kind {
        MemberKind::Column => CompletionItemKind::FIELD,
        MemberKind::Association => CompletionItemKind::PROPERTY,
        MemberKind::Scope => CompletionItemKind::FUNCTION,
        MemberKind::Accessor => CompletionItemKind::PROPERTY,
        MemberKind::Method => CompletionItemKind::METHOD,
    };
    let rank = match m.kind {
        MemberKind::Column | MemberKind::Association | MemberKind::Scope => '0',
        MemberKind::Accessor => '1',
        MemberKind::Method => '2',
    };
    CompletionItem {
        label: m.name.as_str().to_string(),
        kind: Some(kind),
        detail: Some(m.display.clone()),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some(m.display.clone()),
        }),
        sort_text: Some(format!("{rank}{}", m.name.as_str())),
        ..Default::default()
    }
}

/// A type hint after a local-variable binding (`articles = Article.all`
/// → `articles: Array[Article]`). Bounded on purpose: only plain local
/// assignments whose right-hand side is a known *non-literal* type — a
/// literal's type is already obvious in the source, so hinting it is
/// noise.
fn local_assign_hint(expr: &Expr, text: &str) -> Option<InlayHint> {
    let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*expr.node else {
        return None;
    };
    if matches!(&*value.node, ExprNode::Lit { .. }) {
        return None;
    }
    let ty = value.ty.as_ref()?;
    if !is_concrete(ty) {
        return None;
    }
    // The binding name starts at the assignment's span; the hint sits just
    // after it (`name: Ty`).
    let name_end = expr.span.start + name.as_str().len() as u32;
    let position = lsp_position(text, name_end);
    Some(InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {}", ide::render_ty(ty))),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(false),
        data: None,
    })
}

/// A type worth surfacing in a hint — excludes unresolved/gradual types,
/// which would render as `untyped` and add no information.
fn is_concrete(ty: &Ty) -> bool {
    !matches!(ty, Ty::Var { .. } | Ty::Untyped | Ty::Bottom)
}

fn map_severity(sev: Severity) -> DiagnosticSeverity {
    match sev {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        // Gap-attributed coverage notes: faint dots, not squiggles — the
        // finding is about roundhouse's coverage, not the user's code.
        Severity::Info => DiagnosticSeverity::HINT,
    }
}

fn span_to_range(text: &str, span: Span) -> LspRange {
    LspRange { start: lsp_position(text, span.start), end: lsp_position(text, span.end) }
}

fn lsp_position(text: &str, offset: u32) -> LspPosition {
    let p = ide::offset_to_position(text, offset);
    LspPosition { line: p.line, character: p.character }
}

fn lsp_to_ide(p: LspPosition) -> ide::Position {
    ide::Position { line: p.line, character: p.character }
}

/// Canonicalise a path for use as an overlay/diagnostic key. Falls back
/// to the path as-given when it can't be resolved (e.g. an unsaved file),
/// so lookups stay consistent with insertion.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Extract typed params from a request, mapping a transport error into the
/// boxed result type. A method mismatch is a programming error here (we
/// only call this after matching `req.method`), so it surfaces loudly.
fn extract<P: serde::de::DeserializeOwned>(req: Request) -> LspResult<(RequestId, P)> {
    let id = req.id.clone();
    let params = serde_json::from_value(req.params)?;
    Ok((id, params))
}

/// `file://` URI → filesystem path. POSIX-focused: strips the scheme and
/// empty authority, then percent-decodes. (Windows drive URIs aren't
/// handled; the server targets Unix dev hosts.)
fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    let rest = s.strip_prefix("file://")?;
    // Local file URIs have an empty authority, so `rest` is the absolute
    // path starting at '/'. (A non-empty authority would start with a host
    // before the first '/', which we don't expect for local files.)
    Some(PathBuf::from(percent_decode(rest)))
}

/// Filesystem path → `file://` URI. Percent-encodes everything outside
/// the unreserved set, keeping `/` as the separator. POSIX-focused (it
/// assumes an absolute path with `/` separators), matching `uri_to_path`.
fn path_to_uri(path: &Path) -> Option<Uri> {
    let encoded = percent_encode_path(path.to_str()?);
    format!("file://{encoded}").parse().ok()
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Overlay VFS: open buffers shadow disk for content; structure
/// (directory listings, dir-ness) always comes from disk, since open
/// files already exist there.
struct OverlayVfs<'a> {
    disk: FsVfs,
    overlay: &'a HashMap<PathBuf, String>,
}

impl OverlayVfs<'_> {
    fn lookup(&self, path: &Path) -> Option<&String> {
        if let Some(t) = self.overlay.get(path) {
            return Some(t);
        }
        // ingest joins paths from the (possibly non-canonical) root, while
        // overlay keys are canonical — reconcile by canonicalising here.
        let canon = std::fs::canonicalize(path).ok()?;
        self.overlay.get(&canon)
    }
}

impl Vfs for OverlayVfs<'_> {
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        match self.lookup(path) {
            Some(t) => Ok(t.clone().into_bytes()),
            None => self.disk.read(path),
        }
    }

    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        match self.lookup(path) {
            Some(t) => Ok(t.clone()),
            None => self.disk.read_to_string(path),
        }
    }

    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        self.disk.read_dir(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.lookup(path).is_some() || self.disk.exists(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.disk.is_dir(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::{Notification, RequestId};
    use serde_json::json;
    use std::time::Duration;

    /// Read messages off the client end until the response to `id`,
    /// skipping the diagnostics notifications the server interleaves.
    fn recv_response(client: &Connection, id: i32) -> Response {
        let want = RequestId::from(id);
        loop {
            let msg = client
                .receiver
                .recv_timeout(Duration::from_secs(20))
                .expect("server should reply");
            if let Message::Response(resp) = msg {
                if resp.id == want {
                    return resp;
                }
            }
        }
    }

    #[test]
    fn kwarg_call_dot_finds_the_enclosing_call() {
        // Cursor after `(`: resolves to the dot before find_by.
        let t = "x = Article.find_by(";
        assert_eq!(kwarg_call_dot(t, t.len()), Some(t.find(".find_by").unwrap()));
        // Later argument position, past a nested balanced call.
        let t = "Article.where(foo(1), ";
        assert_eq!(kwarg_call_dot(t, t.len()), Some(t.find(".where").unwrap()));
        // Non-kwarg method: no completion context.
        let t = "Article.destroy(";
        assert_eq!(kwarg_call_dot(t, t.len()), None);
        // No receiver dot: bare where( is not resolvable.
        let t = "where(";
        assert_eq!(kwarg_call_dot(t, t.len()), None);
    }

    #[test]
    fn word_start_scans_idents_ivars_and_qualified_constants() {
        let t = "x = @art";
        assert_eq!(word_start(t, t.len()), 4);
        let t = "Admin::Repo";
        assert_eq!(word_start(t, t.len()), 0);
        let t = "user.po";
        assert_eq!(word_start(t, t.len()), 5);
        assert_eq!(word_start(t, 5), 5, "empty word right after the dot");
    }

    /// Drive one completion round: full-sync the buffer to `text`, then
    /// request completion at the position just after the first occurrence
    /// of `marker`. Returns `(label, description)` pairs.
    fn complete_after(
        client: &Connection,
        id: i32,
        uri: &str,
        text: &str,
        marker: &str,
    ) -> Vec<(String, String)> {
        let end = text.find(marker).expect("marker present") + marker.len();
        let line = text[..end].matches('\n').count();
        let line_start = text[..end].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let character = end - line_start;
        client
            .sender
            .send(Message::Notification(Notification {
                method: "textDocument/didChange".to_string(),
                params: json!({
                    "textDocument": { "uri": uri, "version": id },
                    "contentChanges": [{ "text": text }]
                }),
            }))
            .unwrap();
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(id),
                method: "textDocument/completion".to_string(),
                params: json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character }
                }),
            }))
            .unwrap();
        let resp = recv_response(client, id);
        let result = resp.result.expect("completion result");
        if result.is_null() {
            return Vec::new();
        }
        result
            .as_array()
            .expect("completion array")
            .iter()
            .map(|item| {
                (
                    item["label"].as_str().unwrap_or_default().to_string(),
                    item["labelDetails"]["description"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    /// End-to-end completion over the protocol: members after `.`
    /// (class side for a constant, instance side for an ivar), column
    /// kwargs inside `find_by(`, and `@` ivar completion — each answered
    /// from the last-good analysis while the edited buffer is stale.
    #[test]
    fn completion_over_the_protocol() {
        let root = std::env::current_dir().unwrap().join("fixtures/real-blog");
        let path = root.join("app/controllers/articles_controller.rb");
        let content = std::fs::read_to_string(&path).unwrap();
        let uri = format!("file://{}", path.to_str().unwrap());

        let (server, client) = Connection::memory();
        let handle = std::thread::spawn(move || run_connection(server));
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(1),
                method: "initialize".to_string(),
                params: json!({
                    "capabilities": {},
                    "rootUri": format!("file://{}", root.to_str().unwrap()),
                }),
            }))
            .unwrap();
        let _ = recv_response(&client, 1);
        client
            .sender
            .send(Message::Notification(Notification {
                method: "initialized".to_string(),
                params: json!({}),
            }))
            .unwrap();
        // First open analyzes synchronously — completion below reads it.
        client
            .sender
            .send(Message::Notification(Notification {
                method: "textDocument/didOpen".to_string(),
                params: json!({
                    "textDocument": {
                        "uri": uri, "languageId": "ruby", "version": 1, "text": content
                    }
                }),
            }))
            .unwrap();

        // The user types inside `show` — the buffer is now ahead of the
        // analysis. Each probe edits a fresh variant of the file.
        let edit = |line: &str| content.replace("  def show\n  end", &format!("  def show\n{line}\n  end"));

        // Constant receiver → class side: finders and AR class surface,
        // typed against this model; no instance columns.
        let items = complete_after(&client, 2, &uri, &edit("    Article."), "    Article.");
        let find_by = items.iter().find(|(l, _)| l == "find_by");
        assert!(find_by.is_some(), "class side offers find_by; got {} items", items.len());
        assert_eq!(find_by.unwrap().1, "Article?", "find_by is Self-or-nil");
        assert!(items.iter().all(|(l, _)| l != "title"), "columns are instance-side");

        // Kwarg completion inside find_by( → the model's columns.
        let text = edit("    Article.find_by(");
        let items = complete_after(&client, 3, &uri, &text, "find_by(");
        assert!(
            items.iter().any(|(l, _)| l == "title:"),
            "find_by( offers column kwargs; got {items:?}"
        );

        // Ivar receiver on a brand-new line → instance side via the
        // file-ivar fallback (the stale snapshot has no node here).
        let items = complete_after(&client, 4, &uri, &edit("    @article."), "    @article.");
        let title = items.iter().find(|(l, _)| l == "title");
        assert!(title.is_some(), "instance side offers columns; got {} items", items.len());
        assert_eq!(title.unwrap().1, "String");
        assert!(
            items.iter().any(|(l, _)| l == "comments"),
            "instance side offers associations"
        );

        // `@` prefix → ivars known in this file, with types.
        let items = complete_after(&client, 5, &uri, &edit("    @art"), "    @art");
        assert!(
            items.iter().any(|(l, _)| l == "@article"),
            "ivar completion lists @article; got {items:?}"
        );

        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(9),
                method: "shutdown".to_string(),
                params: json!(null),
            }))
            .unwrap();
        let _ = recv_response(&client, 9);
        client
            .sender
            .send(Message::Notification(Notification {
                method: "exit".to_string(),
                params: json!(null),
            }))
            .unwrap();
        handle.join().unwrap().expect("server loop should end cleanly");
    }

    /// End-to-end: stand the server up on an in-memory connection, drive
    /// the real protocol handshake, open a real-blog file, and hover over
    /// a position that ground-truth analysis says is `String`.
    #[test]
    fn hover_reports_inferred_type_over_the_protocol() {
        let root = std::env::current_dir().unwrap().join("fixtures/real-blog");

        // Ground truth (independent of the server): find a `String`-typed
        // byte offset in some `.rb` file, so the probe position is derived
        // from the analyzer, not hard-coded.
        let (ir, _) = crate::ingest::prism::scope(|| crate::ingest::ingest_app(&root));
        let mut app = ir.expect("real-blog should ingest");
        Analyzer::new(&app).analyze(&mut app);

        let mut target: Option<(String, u32, String)> = None;
        'scan: for (i, src) in app.sources.iter().enumerate() {
            if !src.path.ends_with(".rb") {
                continue;
            }
            let file = crate::span::FileId(i as u32 + 1);
            let len = src.text.len() as u32;
            let mut off = 0u32;
            while off < len {
                if let Some(info) = ide::type_at(&app, file, off) {
                    if info.display == "String" {
                        target = Some((src.path.clone(), off, src.text.clone()));
                        break 'scan;
                    }
                }
                off += 1;
            }
        }
        let (path, offset, content) =
            target.expect("real-blog should have a String-typed .rb position");
        let pos = ide::offset_to_position(&content, offset);
        let uri = format!("file://{path}");

        // Stand up the server; drive it as a client.
        let (server, client) = Connection::memory();
        let handle = std::thread::spawn(move || run_connection(server));

        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(1),
                method: "initialize".to_string(),
                params: json!({
                    "capabilities": {},
                    "rootUri": format!("file://{}", root.to_str().unwrap()),
                }),
            }))
            .unwrap();
        let _ = recv_response(&client, 1);
        client
            .sender
            .send(Message::Notification(Notification {
                method: "initialized".to_string(),
                params: json!({}),
            }))
            .unwrap();

        client
            .sender
            .send(Message::Notification(Notification {
                method: "textDocument/didOpen".to_string(),
                params: json!({
                    "textDocument": {
                        "uri": uri, "languageId": "ruby", "version": 1, "text": content
                    }
                }),
            }))
            .unwrap();

        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(2),
                method: "textDocument/hover".to_string(),
                params: json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": pos.line, "character": pos.character }
                }),
            }))
            .unwrap();
        let hover = recv_response(&client, 2);
        let value = hover
            .result
            .expect("hover should have a result")
            .get("contents")
            .and_then(|c| c.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        assert!(value.contains("String"), "hover should report String; got {value:?}");

        // Orderly shutdown so the server thread joins cleanly.
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(3),
                method: "shutdown".to_string(),
                params: json!(null),
            }))
            .unwrap();
        let _ = recv_response(&client, 3);
        client
            .sender
            .send(Message::Notification(Notification {
                method: "exit".to_string(),
                params: json!(null),
            }))
            .unwrap();

        handle.join().unwrap().expect("server loop should end cleanly");
    }
}
