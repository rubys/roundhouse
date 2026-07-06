//! Wasm entry point exposing roundhouse's transpile pipeline as a single
//! `transpile(json) -> json` C-ABI function.
//!
//! Memory protocol — the host (browser JS) does:
//!   1. `rh_alloc(input_len)` → returns a `*mut u8` into wasm linear memory.
//!   2. Write the input JSON (UTF-8) into that buffer.
//!   3. `transpile(ptr, input_len)` → returns a packed `u64` where the low
//!      32 bits are the output ptr and the high 32 bits are the output len.
//!   4. Read the UTF-8 output JSON from wasm memory.
//!   5. `rh_dealloc(input_ptr, input_len)` and `rh_dealloc(out_ptr, out_len)`.
//!
//! Input JSON shape:
//!   `{"language": "typescript", "src": {"app/models/article.rb": "...", ...}}`
//!
//! Output JSON shape (success):
//!   `{"language": "typescript", "files": [{"path": "...", "content": "..."}, ...]}`
//!
//! Output JSON shape (error):
//!   `{"error": "..."}`

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, Severity};
use roundhouse::emit::ruby::ty_to_rbs;
use roundhouse::emit::{crystal, csharp, elixir, go, kotlin, python, ruby, rust, swift, typescript};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::profile::DeploymentProfile;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct TranspileInput {
    language: String,
    src: HashMap<String, String>,
    /// Optional deployment profile, only meaningful for `typescript`:
    /// `worker` (SharedWorker browser app — what /studio/ runs), `node-async`,
    /// `node-sync`. Absent/`default` ⇒ the plain `emit()` (what /playground/
    /// shows). Ignored by every other target.
    #[serde(default)]
    profile: Option<String>,
}

#[derive(Serialize)]
struct TranspileOutput<'a> {
    language: &'a str,
    files: Vec<EmittedFile>,
    /// Analyzer diagnostics (inference results) attributed to source
    /// positions. Target-independent — the same Ruby types (or fails to)
    /// regardless of the emit backend, so this is identical across targets.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<DiagnosticOut>,
    /// Inferred type at each source expression (RBS string form), for hovers.
    /// Also target-independent. Unresolved placeholders (`Ty::Var`) are
    /// dropped; everything else (incl. the gradual `untyped`) is kept.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inferred_types: Vec<TypeOut>,
}

#[derive(Serialize)]
struct EmittedFile {
    path: String,
    content: String,
}

/// A diagnostic resolved to a 1-based source location (UTF-8 char columns),
/// ready for the playground to drop as a Monaco marker on the source file.
#[derive(Serialize)]
struct DiagnosticOut {
    path: String,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
    severity: &'static str,
    code: &'static str,
    message: String,
}

/// An inferred type at a 1-based source span, rendered to its RBS string.
#[derive(Serialize)]
struct TypeOut {
    path: String,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
    ty: String,
}

#[derive(Serialize)]
struct ErrorOutput {
    error: String,
}

fn transpile_inner(json_in: &str) -> String {
    let input: TranspileInput = match serde_json::from_str(json_in) {
        Ok(v) => v,
        Err(e) => return error_json(&format!("invalid input JSON: {e}")),
    };

    let tree: HashMap<PathBuf, Vec<u8>> = input
        .src
        .into_iter()
        .map(|(k, v)| (PathBuf::from(k), v.into_bytes()))
        .collect();

    let mut app = match ingest_app_from_tree(tree) {
        Ok(app) => app,
        Err(e) => return error_json(&format!("ingest: {e}")),
    };

    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    // Keep the member tables: transpile stashes the same last-good
    // snapshot the query surface answers from (see below), so the
    // playground/studio get completion as a free byproduct of the
    // analysis they already run per edit.
    let registry = analyzer.class_registry().clone();

    // Surface analyzer diagnostics, resolved to source positions. Synthetic
    // spans (no source site) are dropped — there's nowhere to put a marker.
    let diagnostics: Vec<DiagnosticOut> = diagnose(&app)
        .into_iter()
        .filter_map(|d| {
            if d.span.is_synthetic() {
                return None;
            }
            let sf = app.sources.get((d.span.file.0 as usize).checked_sub(1)?)?;
            let (start_line, start_col) = sf.line_col(d.span.start);
            let (end_line, end_col) = sf.line_col(d.span.end);
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                // Gap-attributed coverage notes (analyze::attribution).
                // The playground/studio ingest strict (no survey mode),
                // so these don't occur there today; mapped for
                // completeness against a future survey-mode wiring.
                Severity::Info => "info",
            };
            let code = d.code();
            Some(DiagnosticOut {
                path: sf.path.clone(),
                start_line,
                start_col,
                end_line,
                end_col,
                severity,
                code,
                message: d.message,
            })
        })
        .collect();

    // Inferred type at each source expression, for hover tooltips. Drop
    // unresolved `Ty::Var` placeholders (they'd render a misleading
    // "untyped") and anything without a real source span.
    let inferred_types: Vec<TypeOut> = roundhouse::analyze::inferred_types(&app)
        .into_iter()
        .filter_map(|(span, ty)| {
            if span.is_synthetic() || matches!(ty, roundhouse::ty::Ty::Var { .. }) {
                return None;
            }
            let sf = app.sources.get((span.file.0 as usize).checked_sub(1)?)?;
            let (start_line, start_col) = sf.line_col(span.start);
            let (end_line, end_col) = sf.line_col(span.end);
            Some(TypeOut {
                path: sf.path.clone(),
                start_line,
                start_col,
                end_line,
                end_col,
                ty: ty_to_rbs(&ty),
            })
        })
        .collect();

    let emitted = match input.language.as_str() {
        "typescript" | "ts" => match input.profile.as_deref() {
            Some("worker") => typescript::emit_with_profile(&app, &DeploymentProfile::worker()),
            Some("node-async") => {
                typescript::emit_with_profile(&app, &DeploymentProfile::node_async())
            }
            Some("node-sync") => {
                typescript::emit_with_profile(&app, &DeploymentProfile::node_sync())
            }
            None | Some("default") => typescript::emit(&app),
            Some(other) => return error_json(&format!("unknown profile: {other}")),
        },
        "rust" | "rs" => rust::emit(&app),
        "crystal" | "cr" => crystal::emit(&app),
        "python" | "py" => python::emit(&app),
        "elixir" | "ex" => elixir::emit(&app),
        "go" => go::emit(&app),
        "kotlin" | "kt" => kotlin::emit(&app),
        "swift" | "sw" => swift::emit(&app),
        "csharp" | "cs" => csharp::emit(&app),
        // Ruby/spinel's aggregate emitter is `emit_spinel` (legacy name); it
        // returns the full project (.rb + .rbs sidecars) like the others' emit().
        "ruby" | "spinel" => ruby::emit_spinel(&app),
        other => return error_json(&format!("unknown language: {other}")),
    };

    let files: Vec<EmittedFile> = emitted
        .into_iter()
        .map(|f| EmittedFile {
            path: f.path.display().to_string(),
            content: f.content,
        })
        .collect();

    let out = TranspileOutput {
        language: &input.language,
        files,
        diagnostics,
        inferred_types,
    };

    let json =
        serde_json::to_string(&out).unwrap_or_else(|e| error_json(&format!("serialize: {e}")));
    // Refresh the query snapshot last — every borrow of `app` above has
    // ended, and `complete`/`type_at` now answer against exactly the
    // analysis this transpile ran.
    LAST_GOOD.with(|l| *l.borrow_mut() = Some(Analysis { app, registry }));
    json
}

fn error_json(msg: &str) -> String {
    serde_json::to_string(&ErrorOutput {
        error: msg.to_string(),
    })
    .unwrap_or_else(|_| String::from(r#"{"error":"unserializable error"}"#))
}

// ── Analysis-only entry (the IDE skin) ───────────────────────────────
//
// The /ide/ page doesn't emit code; it queries. `analyze` ingests in
// *survey mode* (real apps always have constructs the subset doesn't
// model — the gaps drive note-severity attribution, exactly like the
// LSP/MCP skins), runs the whole-app analysis, publishes the marker
// list, and stashes the typed App + class registry in a thread-local
// as the **last-good snapshot** that `complete` (and future queries)
// answer from. Wasm is single-threaded, so the thread-local is the
// whole story — no locks, no worker plumbing on this side.

use std::cell::RefCell;

struct Analysis {
    app: roundhouse::App,
    registry: HashMap<roundhouse::ClassId, roundhouse::analyze::ClassInfo>,
}

thread_local! {
    static LAST_GOOD: RefCell<Option<Analysis>> = const { RefCell::new(None) };
}

#[derive(Deserialize)]
struct AnalyzeInput {
    src: HashMap<String, String>,
}

#[derive(Serialize)]
struct AnalyzeOutput {
    /// Marker-ready diagnostics (notes carry severity "info").
    diagnostics: Vec<DiagnosticOut>,
    /// Recovered ingest gaps (file + message) — the coverage ledger.
    gaps: Vec<GapOut>,
    /// Ingested source files (path list, for pickers).
    files: Vec<String>,
    /// Registered classes (models/controllers/concerns/lib), for
    /// go-to-class search.
    classes: Vec<String>,
}

#[derive(Serialize)]
struct GapOut {
    file: String,
    message: String,
}

fn analyze_app_inner(json_in: &str) -> String {
    let input: AnalyzeInput = match serde_json::from_str(json_in) {
        Ok(v) => v,
        Err(e) => return error_json(&format!("invalid input JSON: {e}")),
    };
    let tree: HashMap<PathBuf, Vec<u8>> = input
        .src
        .into_iter()
        .map(|(k, v)| (PathBuf::from(k), v.into_bytes()))
        .collect();

    roundhouse::ingest::survey::activate();
    let (result, parse_diags) =
        roundhouse::ingest::prism::scope(|| ingest_app_from_tree(tree));
    let gaps = roundhouse::ingest::survey::drain();
    let mut app = match result {
        Ok(app) => app,
        Err(e) => return error_json(&format!("ingest: {e}")),
    };
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let registry = analyzer.class_registry().clone();

    let mut diags = diagnose(&app);
    roundhouse::analyze::attribution::attribute_ingest_gaps(&mut diags, &app, &gaps);
    diags.extend(parse_diags);

    let diagnostics: Vec<DiagnosticOut> = diags
        .into_iter()
        .filter_map(|d| {
            if d.span.is_synthetic() {
                return None;
            }
            let sf = app.sources.get((d.span.file.0 as usize).checked_sub(1)?)?;
            let (start_line, start_col) = sf.line_col(d.span.start);
            let (end_line, end_col) = sf.line_col(d.span.end);
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            Some(DiagnosticOut {
                path: sf.path.clone(),
                start_line,
                start_col,
                end_line,
                end_col,
                severity,
                code: d.code(),
                message: d.message,
            })
        })
        .collect();

    let gaps_out: Vec<GapOut> = gaps
        .iter()
        .filter_map(|g| match g {
            roundhouse::ingest::IngestError::Unsupported { file, message }
            | roundhouse::ingest::IngestError::Parse { file, message } => Some(GapOut {
                file: file.clone(),
                message: message.clone(),
            }),
            _ => None,
        })
        .collect();

    let files: Vec<String> = app.sources.iter().map(|s| s.path.clone()).collect();
    let mut classes: Vec<String> =
        registry.keys().map(|c| c.0.as_str().to_string()).collect();
    classes.sort();

    let out = AnalyzeOutput { diagnostics, gaps: gaps_out, files, classes };
    let json =
        serde_json::to_string(&out).unwrap_or_else(|e| error_json(&format!("serialize: {e}")));
    LAST_GOOD.with(|l| *l.borrow_mut() = Some(Analysis { app, registry }));
    json
}

/// Pack a String into the (ptr,len) u64 protocol shared with `transpile`.
fn pack(result: String) -> u64 {
    let bytes = result.into_bytes();
    let len = bytes.len() as u64;
    let mut boxed = bytes.into_boxed_slice();
    let ptr = boxed.as_mut_ptr() as u64;
    std::mem::forget(boxed);
    (ptr & 0xFFFF_FFFF) | (len << 32)
}

/// Analysis-only entry: same memory protocol as `transpile`, input
/// `{"src": {...}}`, output `AnalyzeOutput` (or `{"error": ...}`).
/// Side effect: refreshes the last-good snapshot queries answer from.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn analyze_app(input_ptr: *const u8, input_len: u32) -> u64 {
    let input = unsafe { std::slice::from_raw_parts(input_ptr, input_len as usize) };
    let json_in = std::str::from_utf8(input).unwrap_or("{}");
    pack(analyze_app_inner(json_in))
}

// ── Queries over the last-good snapshot ──────────────────────────────

#[derive(Deserialize)]
struct CompleteInput {
    path: String,
    /// The *current* buffer text — ahead of the analyzed snapshot by
    /// the keystroke(s) that triggered the request, same contract as
    /// the LSP skin.
    text: String,
    line: u32,
    character: u32,
}

#[derive(Serialize)]
struct CandidateOut {
    label: String,
    kind: &'static str,
    detail: String,
    sort_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    insert_text: Option<String>,
}

fn complete_inner(json_in: &str) -> String {
    let input: CompleteInput = match serde_json::from_str(json_in) {
        Ok(v) => v,
        Err(e) => return error_json(&format!("invalid input JSON: {e}")),
    };
    LAST_GOOD.with(|l| {
        let borrow = l.borrow();
        let Some(a) = borrow.as_ref() else {
            return error_json("no analysis yet — call analyze_app first");
        };
        let pos = roundhouse::ide::Position { line: input.line, character: input.character };
        let offset = roundhouse::ide::position_to_offset(&input.text, pos) as usize;
        let cands = roundhouse::ide::complete_at(&a.app, &a.registry, &input.path, &input.text, offset)
            .unwrap_or_default();
        let out: Vec<CandidateOut> = cands
            .into_iter()
            .map(|c| CandidateOut {
                sort_text: format!("{}{}", c.rank, c.label),
                kind: match c.kind {
                    roundhouse::ide::CandidateKind::Column => "column",
                    roundhouse::ide::CandidateKind::Association => "association",
                    roundhouse::ide::CandidateKind::Scope => "scope",
                    roundhouse::ide::CandidateKind::Accessor => "accessor",
                    roundhouse::ide::CandidateKind::Method => "method",
                    roundhouse::ide::CandidateKind::Ivar => "ivar",
                    roundhouse::ide::CandidateKind::Kwarg => "kwarg",
                },
                label: c.label,
                detail: c.detail,
                insert_text: c.insert_text,
            })
            .collect();
        serde_json::to_string(&out).unwrap_or_else(|e| error_json(&format!("serialize: {e}")))
    })
}

/// Completion against the last-good snapshot + the passed current
/// buffer. Input `{"path", "text", "line", "character"}` (0-based,
/// UTF-16 character — Monaco/LSP convention), output a candidate array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn complete(input_ptr: *const u8, input_len: u32) -> u64 {
    let input = unsafe { std::slice::from_raw_parts(input_ptr, input_len as usize) };
    pack(complete_inner(std::str::from_utf8(input).unwrap_or("{}")))
}

#[derive(Deserialize)]
struct PositionInput {
    path: String,
    line: u32,
    character: u32,
}

#[derive(Serialize)]
struct TypeAtOut {
    display: String,
    nilable: bool,
    node_kind: &'static str,
}

fn type_at_inner(json_in: &str) -> String {
    let input: PositionInput = match serde_json::from_str(json_in) {
        Ok(v) => v,
        Err(e) => return error_json(&format!("invalid input JSON: {e}")),
    };
    LAST_GOOD.with(|l| {
        let borrow = l.borrow();
        let Some(a) = borrow.as_ref() else {
            return error_json("no analysis yet — call analyze_app first");
        };
        let pos = roundhouse::ide::Position { line: input.line, character: input.character };
        match roundhouse::ide::type_at_position(&a.app, &input.path, pos) {
            Some(info) => serde_json::to_string(&TypeAtOut {
                display: info.display,
                nilable: info.nilable,
                node_kind: info.node_kind,
            })
            .unwrap_or_else(|e| error_json(&format!("serialize: {e}"))),
            None => "null".to_string(),
        }
    })
}

/// Hover: inferred type at a position in the *analyzed* text (the
/// snapshot's own source — positions drift at most one edit, exactly
/// like the LSP's hover). Output `{"display","nilable","node_kind"}`
/// or `null`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_at(input_ptr: *const u8, input_len: u32) -> u64 {
    let input = unsafe { std::slice::from_raw_parts(input_ptr, input_len as usize) };
    pack(type_at_inner(std::str::from_utf8(input).unwrap_or("{}")))
}

#[derive(Deserialize)]
struct RelatedInput {
    path: String,
}

#[derive(Serialize)]
struct RelatedOut {
    kind: &'static str,
    label: String,
    path: String,
}

fn related_files_inner(json_in: &str) -> String {
    let input: RelatedInput = match serde_json::from_str(json_in) {
        Ok(v) => v,
        Err(e) => return error_json(&format!("invalid input JSON: {e}")),
    };
    LAST_GOOD.with(|l| {
        let borrow = l.borrow();
        let Some(a) = borrow.as_ref() else {
            return error_json("no analysis yet — call analyze_app first");
        };
        let rel = roundhouse::ide::related_files(&a.app, &input.path);
        let out: Vec<RelatedOut> = rel
            .into_iter()
            .map(|r| RelatedOut {
                kind: match r.kind {
                    roundhouse::ide::RelatedKind::View => "view",
                    roundhouse::ide::RelatedKind::Partial => "partial",
                    roundhouse::ide::RelatedKind::Renderer => "renderer",
                    roundhouse::ide::RelatedKind::Controller => "controller",
                    roundhouse::ide::RelatedKind::Concern => "concern",
                    roundhouse::ide::RelatedKind::Includer => "includer",
                    roundhouse::ide::RelatedKind::Model => "model",
                },
                label: r.label,
                path: r.path,
            })
            .collect();
        serde_json::to_string(&out).unwrap_or_else(|e| error_json(&format!("serialize: {e}")))
    })
}

/// Rails-aware related files for a path, from the inferred render
/// graph + include edges. Input `{"path"}`, output an array of
/// `{"kind","label","path"}`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn related_files(input_ptr: *const u8, input_len: u32) -> u64 {
    let input = unsafe { std::slice::from_raw_parts(input_ptr, input_len as usize) };
    pack(related_files_inner(std::str::from_utf8(input).unwrap_or("{}")))
}

// ── C ABI exports ────────────────────────────────────────────────────

/// Allocate a buffer of the given size in wasm linear memory and return
/// a pointer to it. Caller is responsible for calling `rh_dealloc`.
#[unsafe(no_mangle)]
pub extern "C" fn rh_alloc(size: u32) -> *mut u8 {
    let mut v: Vec<u8> = Vec::with_capacity(size as usize);
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    ptr
}

/// Free a buffer previously returned by `rh_alloc` or `transpile`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_dealloc(ptr: *mut u8, size: u32) {
    if ptr.is_null() || size == 0 {
        return;
    }
    let _ = unsafe { Vec::from_raw_parts(ptr, 0, size as usize) };
}

/// Run the transpile pipeline on a UTF-8 JSON input. Returns a packed
/// `(ptr, len)` in a single `u64` — the low 32 bits are the pointer to
/// the result buffer, the high 32 bits are its length in bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn transpile(input_ptr: *const u8, input_len: u32) -> u64 {
    let input = unsafe { std::slice::from_raw_parts(input_ptr, input_len as usize) };
    let json_in = std::str::from_utf8(input).unwrap_or("{}");
    let result = transpile_inner(json_in);

    let bytes = result.into_bytes();
    let len = bytes.len() as u64;
    let mut boxed = bytes.into_boxed_slice();
    let ptr = boxed.as_mut_ptr() as u64;
    std::mem::forget(boxed);

    (ptr & 0xFFFF_FFFF) | (len << 32)
}
