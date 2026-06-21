//! Standalone read-only LSP server — Rung 1 of roundhouse#57.
//!
//! Wires the [`crate::ide`] query layer to editors over the Language
//! Server Protocol. Read-only by design: it publishes diagnostics and
//! answers `hover` / `inlayHint`, and never proposes edits — so the
//! server is a pure analysis process with no filesystem side effects
//! (it reads the workspace; the editor owns every buffer).
//!
//! Transport is the *synchronous* `lsp-server` crate (rust-analyzer's),
//! deliberately: the analysis engine is sync and fast (~sub-second
//! whole-app), so there is no async runtime here and no incrementality —
//! every change re-ingests and re-analyses the whole app through an
//! overlay VFS that layers open buffers on top of disk. The 0.55s
//! whole-app figure (see [`crate::ide`]) is what makes full-document
//! sync viable.
//!
//! ## Model
//!
//! The analyzer ingests a whole Rails app, not a file. So the server
//! keeps the workspace root, an overlay of open buffers (path → current
//! text), and the last analysed [`App`]. On every `didOpen`/`didChange`/
//! `didClose` it rebuilds the overlay, re-ingests + re-analyses, then
//! publishes diagnostics for the open documents and refreshes the cached
//! `App` that `hover`/`inlayHint` query.

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use lsp_server::{Connection, Message, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    GotoDefinition, HoverRequest, InlayHintRequest, References, Request as _,
};
use lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, InitializeParams, InlayHint,
    InlayHintKind, InlayHintLabel, InlayHintParams, Location, MarkupContent, MarkupKind, OneOf,
    Position as LspPosition, PublishDiagnosticsParams, Range as LspRange, ReferenceParams,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use crate::analyze::{diagnose, Analyzer, Severity};
use crate::app::App;
use crate::diagnostic::{Diagnostic as RhDiagnostic, DiagnosticKind};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ide;
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

struct Server {
    connection: Connection,
    root: PathBuf,
    /// Open buffers, keyed by canonical absolute path → current text.
    overlay: HashMap<PathBuf, String>,
    /// Open document URIs, in open order — the set we publish (and clear)
    /// diagnostics for each analysis pass.
    open: Vec<Uri>,
    /// Last app that ingested+analysed cleanly; what hover/inlay query.
    app: Option<App>,
}

impl Server {
    fn new(connection: Connection, root: PathBuf) -> Self {
        Self { connection, root, overlay: HashMap::new(), open: Vec::new(), app: None }
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
            self.reanalyze_and_publish()?;
        } else if not.method == DidChangeTextDocument::METHOD {
            let p: DidChangeTextDocumentParams = serde_json::from_value(not.params)?;
            // Full sync: the last change carries the whole document.
            if let Some(change) = p.content_changes.into_iter().next_back() {
                self.set_buffer(&p.text_document.uri, change.text);
            }
            self.reanalyze_and_publish()?;
        } else if not.method == DidCloseTextDocument::METHOD {
            let p: DidCloseTextDocumentParams = serde_json::from_value(not.params)?;
            if let Some(path) = uri_to_path(&p.text_document.uri) {
                self.overlay.remove(&canonical(&path));
            }
            self.open.retain(|u| u != &p.text_document.uri);
            // Clear diagnostics for the file we no longer track.
            self.publish_for(&p.text_document.uri, Vec::new())?;
            self.reanalyze_and_publish()?;
        }
        Ok(())
    }

    fn set_buffer(&mut self, uri: &Uri, text: String) {
        if let Some(path) = uri_to_path(uri) {
            self.overlay.insert(canonical(&path), text);
        }
    }

    /// Re-ingest + re-analyse the whole app through the overlay, refresh
    /// the cached `App`, and publish diagnostics for every open document.
    fn reanalyze_and_publish(&mut self) -> LspResult<()> {
        let diags = self.reanalyze();
        self.publish(diags)
    }

    fn reanalyze(&mut self) -> Vec<RhDiagnostic> {
        let vfs = OverlayVfs { disk: FsVfs::new(), overlay: &self.overlay };
        let (result, mut parse_diags) =
            crate::ingest::prism::scope(|| ingest_app_with_vfs(&vfs, &self.root));
        match result {
            Ok(mut app) => {
                Analyzer::new(&app).analyze(&mut app);
                let mut diags = diagnose(&app);
                diags.append(&mut parse_diags);
                self.app = Some(app);
                diags
            }
            Err(err) => {
                // Prism's error recovery means a hard ingest failure is
                // rare; keep the last good `App` for hover and surface
                // whatever parse errors we captured.
                eprintln!("roundhouse-lsp: ingest failed: {err}");
                parse_diags
            }
        }
    }

    fn publish(&self, diags: Vec<RhDiagnostic>) -> LspResult<()> {
        // Group by source path, then convert. Grouping on the roundhouse
        // diagnostics (pre-conversion) lets us apply the mid-edit
        // suppression heuristic by kind.
        let mut by_path: HashMap<PathBuf, Vec<RhDiagnostic>> = HashMap::new();
        if let Some(app) = &self.app {
            for d in diags {
                if let Some(src) = ide::source(app, d.span.file) {
                    by_path.entry(canonical(Path::new(&src.path))).or_default().push(d);
                }
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
        for uri in &self.open {
            let items = uri_to_path(uri)
                .and_then(|p| by_path.get(&canonical(&p)))
                .map(|ds| self.convert_diags(ds))
                .unwrap_or_default();
            self.publish_for(uri, items)?;
        }
        Ok(())
    }

    fn convert_diags(&self, diags: &[RhDiagnostic]) -> Vec<LspDiagnostic> {
        let Some(app) = &self.app else { return Vec::new() };
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

    fn publish_for(&self, uri: &Uri, diagnostics: Vec<LspDiagnostic>) -> LspResult<()> {
        let params = PublishDiagnosticsParams { uri: uri.clone(), diagnostics, version: None };
        let not = lsp_server::Notification::new(PublishDiagnostics::METHOD.to_string(), params);
        self.connection.sender.send(Message::Notification(not))?;
        Ok(())
    }

    fn hover(&self, params: HoverParams) -> Option<Hover> {
        let app = self.app.as_ref()?;
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
        let Some(app) = self.app.as_ref() else { return Vec::new() };
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
        let app = self.app.as_ref()?;
        let tdp = params.text_document_position;
        let path = uri_to_path(&tdp.text_document.uri)?;
        let file = ide::file_id(app, path.to_str()?)?;
        let src = ide::source(app, file)?;
        let offset = ide::position_to_offset(&src.text, lsp_to_ide(tdp.position));
        let include_decl = params.context.include_declaration;
        let decl = ide::definition(app, file, offset);
        let locations = ide::references(app, file, offset)
            .into_iter()
            .filter(|r| include_decl || Some(r.span) != decl)
            .filter_map(|r| self.location_of(r.span))
            .collect();
        Some(locations)
    }

    fn goto_definition(&self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let app = self.app.as_ref()?;
        let tdp = params.text_document_position_params;
        let path = uri_to_path(&tdp.text_document.uri)?;
        let file = ide::file_id(app, path.to_str()?)?;
        let src = ide::source(app, file)?;
        let offset = ide::position_to_offset(&src.text, lsp_to_ide(tdp.position));
        let span = ide::definition(app, file, offset)?;
        Some(GotoDefinitionResponse::Scalar(self.location_of(span)?))
    }

    /// An LSP `Location` for a span — its file as a `file://` URI plus the
    /// UTF-16 range. `None` for the synthetic file or an unencodable path.
    fn location_of(&self, span: Span) -> Option<Location> {
        let app = self.app.as_ref()?;
        let src = ide::source(app, span.file)?;
        Some(Location { uri: path_to_uri(Path::new(&src.path))?, range: span_to_range(&src.text, span) })
    }

    fn respond<T: serde::Serialize>(&self, id: RequestId, result: &T) -> LspResult<()> {
        let resp = Response { id, result: Some(serde_json::to_value(result)?), error: None };
        self.connection.sender.send(Message::Response(resp))?;
        Ok(())
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
