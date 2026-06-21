//! IDE/agent query layer — Rung 0 of roundhouse#57.
//!
//! Turns the *batch* whole-app analysis into a *position-addressable*
//! surface: given a file and a byte offset (or an LSP line:character
//! position), what is the inferred type here, can it be `nil`, and which
//! syntactic node is it? This is the shared substrate every protocol
//! wiring sits on — the standalone LSP server, the MCP tool skin, and the
//! ruby-lsp add-on all consume these functions. It deliberately holds no
//! protocol, transport, or I/O concerns of its own.
//!
//! Precondition: call this on an [`App`] that has already been through
//! [`crate::analyze::Analyzer::analyze`], so every reachable `Expr.ty` is
//! populated. The spans consulted here are the *ingest* (pre-lowering)
//! spans, which point back into [`App::sources`]; do not run a lowering
//! pass before querying or the offsets stop matching the user's buffer.
//!
//! The position math is UTF-16 aware (LSP measures `character` in UTF-16
//! code units, not bytes or Unicode scalars), so multi-byte and astral
//! characters land where the editor expects.

use crate::app::App;
use crate::expr::Expr;
use crate::span::{FileId, SourceFile, Span};
use crate::ty::{Row, Ty};

/// A zero-based LSP-style position: `line` plus a UTF-16 `character`
/// offset into that line. (LSP's default `positionEncoding`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// The answer to a position query: the tightest typed node covering an
/// offset, plus a consumer-facing view of its inferred type.
#[derive(Clone, Debug)]
pub struct TypeAt {
    /// Span of the matched node (always within the queried file).
    pub span: Span,
    /// Syntactic kind of the matched node (`"Send"`, `"Ivar"`, `"Lit"`…),
    /// from [`ExprNode::kind_str`].
    pub node_kind: &'static str,
    /// The inferred type, when the analyzer resolved one. `None` when the
    /// node was never typed (`Expr.ty == None`).
    pub ty: Option<Ty>,
    /// Human-facing, RBS-flavoured rendering of `ty` (`"String"`,
    /// `"Article?"`, `"Array[Integer]"`). `"untyped"` when the type is
    /// absent or an unresolved inference variable.
    pub display: String,
    /// Whether a value here can be `nil` — the type is `nil`, or a union
    /// with a `nil` arm. Drives nil-safety underlines. Conservatively
    /// `false` for unknown/untyped positions: we underline only types we
    /// can *prove* admit nil.
    pub nilable: bool,
}

/// Resolve a source path to its [`FileId`]. Matches the stored path
/// exactly first, then falls back to a suffix match in either direction
/// so a caller can pass an app-relative path even though `App::sources`
/// carry the ingest-root prefix (and vice-versa). `None` when no source
/// matches.
pub fn file_id(app: &App, path: &str) -> Option<FileId> {
    if let Some(i) = app.sources.iter().position(|s| s.path == path) {
        return Some(FileId(i as u32 + 1));
    }
    app.sources
        .iter()
        .position(|s| s.path.ends_with(path) || path.ends_with(s.path.as_str()))
        .map(|i| FileId(i as u32 + 1))
}

/// The [`SourceFile`] behind a [`FileId`], or `None` for the synthetic
/// sentinel (`FileId(0)`) and out-of-range ids.
pub fn source(app: &App, file: FileId) -> Option<&SourceFile> {
    if file.0 == 0 {
        return None;
    }
    app.sources.get(file.0 as usize - 1)
}

/// Inferred type at a byte `offset` within `file`: the tightest
/// (smallest-span) typed node covering the offset, described for a
/// consumer. `None` when the offset lands outside every node (whitespace
/// between top-level forms, a comment, or past EOF).
pub fn type_at(app: &App, file: FileId, offset: u32) -> Option<TypeAt> {
    find_at_offset(app, file, offset).map(describe)
}

/// Convenience for protocol wirings: resolve a source `path` and an LSP
/// `pos` to a type query in one call. `None` if the path is not a known
/// source.
pub fn type_at_position(app: &App, path: &str, pos: Position) -> Option<TypeAt> {
    let file = file_id(app, path)?;
    let text = &source(app, file)?.text;
    let offset = position_to_offset(text, pos);
    type_at(app, file, offset)
}

/// The tightest [`Expr`] whose span covers `offset` in `file`. Implemented
/// as a full walk that keeps the smallest containing span — robust to
/// synthetic-span intermediate nodes, since a synthetic span never
/// contains a real offset and is simply skipped. On exactly-equal spans
/// the deeper node wins (pre-order visits a parent before its children,
/// and an equal-or-smaller span replaces the incumbent).
pub fn find_at_offset(app: &App, file: FileId, offset: u32) -> Option<&Expr> {
    let mut best: Option<&Expr> = None;
    for root in root_bodies(app) {
        walk(root, &mut |e| {
            if covers(e.span, file, offset) {
                match best {
                    Some(b) if e.span.len() > b.span.len() => {}
                    _ => best = Some(e),
                }
            }
        });
    }
    best
}

fn covers(span: Span, file: FileId, offset: u32) -> bool {
    span.file == file && span.start <= offset && offset < span.end
}

fn describe(expr: &Expr) -> TypeAt {
    TypeAt {
        span: expr.span,
        node_kind: expr.node.kind_str(),
        display: match &expr.ty {
            Some(ty) => render_ty(ty),
            None => "untyped".to_string(),
        },
        nilable: expr.ty.as_ref().is_some_and(can_be_nil),
        ty: expr.ty.clone(),
    }
}

/// Whether a value of this type can be `nil`: the type is `nil`, or a
/// union with a `nil` arm (transitively). Unknown / untyped / gradual
/// positions return `false` — we only claim nilability we can prove.
pub fn can_be_nil(ty: &Ty) -> bool {
    match ty {
        Ty::Nil => true,
        Ty::Union { variants } => variants.iter().any(can_be_nil),
        _ => false,
    }
}

/// Render a [`Ty`] as a short, RBS-flavoured string for hover/inlay/MCP
/// output. Ruby developers read `Integer`/`String`/`Article?`, not the
/// IR's `Int`/`Str`/`Union`, so this is the consumer-facing projection —
/// distinct from the IR's `Debug` and from the strict `.rbs` emitter.
pub fn render_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int => "Integer".to_string(),
        Ty::Float => "Float".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Str => "String".to_string(),
        Ty::Sym => "Symbol".to_string(),
        Ty::Nil => "nil".to_string(),
        Ty::Array { elem } => format!("Array[{}]", render_ty(elem)),
        Ty::Hash { key, value } => format!("Hash[{}, {}]", render_ty(key), render_ty(value)),
        Ty::Tuple { elems } => format!("[{}]", render_list(elems)),
        Ty::Record { row } => render_record(row),
        Ty::Union { variants } => render_union(variants),
        Ty::Class { id, args } => {
            if args.is_empty() {
                id.to_string()
            } else {
                format!("{}[{}]", id, render_list(args))
            }
        }
        Ty::Fn { params, ret, .. } => {
            let ps = params.iter().map(|p| render_ty(&p.ty)).collect::<Vec<_>>().join(", ");
            format!("^({}) -> {}", ps, render_ty(ret))
        }
        // An unresolved inference variable reads as "untyped" to a
        // consumer — same bucket the diagnostics walker treats as unknown.
        Ty::Var { .. } => "untyped".to_string(),
        Ty::Untyped => "untyped".to_string(),
        Ty::Bottom => "bot".to_string(),
    }
}

fn render_list(tys: &[Ty]) -> String {
    tys.iter().map(render_ty).collect::<Vec<_>>().join(", ")
}

/// `A | nil` collapses to RBS's `A?`; wider unions render `A | B | nil`.
fn render_union(variants: &[Ty]) -> String {
    let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
    let non_nil: Vec<&Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).collect();
    if has_nil && non_nil.len() == 1 {
        return format!("{}?", render_ty(non_nil[0]));
    }
    let mut parts: Vec<String> = non_nil.into_iter().map(render_ty).collect();
    if has_nil {
        parts.push("nil".to_string());
    }
    if parts.is_empty() {
        "nil".to_string()
    } else {
        parts.join(" | ")
    }
}

fn render_record(row: &Row) -> String {
    let mut fields: Vec<String> =
        row.fields.iter().map(|(k, v)| format!("{}: {}", k, render_ty(v))).collect();
    if row.rest.is_some() {
        fields.push("...".to_string());
    }
    if fields.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", fields.join(", "))
    }
}

/// Byte offset → zero-based UTF-16 [`Position`]. Offsets past EOF clamp to
/// the end, matching how editors keep sending stale positions mid-edit.
pub fn offset_to_position(text: &str, offset: u32) -> Position {
    let offset = (offset as usize).min(text.len());
    let before = &text.as_bytes()[..offset];
    let line = before.iter().filter(|&&b| b == b'\n').count() as u32;
    let line_start = before.iter().rposition(|&b| b == b'\n').map(|p| p + 1).unwrap_or(0);
    let character = text[line_start..offset].chars().map(|c| c.len_utf16() as u32).sum();
    Position { line, character }
}

/// Zero-based UTF-16 [`Position`] → byte offset. A `line` past EOF clamps
/// to `text.len()`; a `character` past end-of-line clamps to the line's
/// terminator — both deliberate, so a position from a buffer the analyzer
/// hasn't caught up to never panics or wraps.
pub fn position_to_offset(text: &str, pos: Position) -> u32 {
    // Byte index of the start of `pos.line`.
    let mut line_start = 0usize;
    if pos.line > 0 {
        let mut seen = 0u32;
        let mut found = false;
        for (i, &b) in text.as_bytes().iter().enumerate() {
            if b == b'\n' {
                seen += 1;
                if seen == pos.line {
                    line_start = i + 1;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return text.len() as u32;
        }
    }
    // Walk UTF-16 units across the line until we reach `pos.character`.
    let mut units = 0u32;
    for (off, c) in text[line_start..].char_indices() {
        if units >= pos.character || c == '\n' {
            return (line_start + off) as u32;
        }
        units += c.len_utf16() as u32;
    }
    text.len() as u32
}

/// Every root body in the app whose subtree the analyzer types — the same
/// set [`crate::analyze::diagnose`] walks: controller actions, model
/// scopes and methods, view bodies, and `db/seeds.rb`.
fn root_bodies(app: &App) -> Vec<&Expr> {
    let mut roots = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            roots.push(&action.body);
        }
    }
    for model in &app.models {
        for scope in model.scopes() {
            roots.push(&scope.body);
        }
        for method in model.methods() {
            roots.push(&method.body);
        }
    }
    for view in &app.views {
        roots.push(&view.body);
    }
    if let Some(seeds) = &app.seeds {
        roots.push(seeds);
    }
    roots
}

/// Pre-order subtree walk: visit `e`, then recurse into its children.
fn walk<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| walk(c, f));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprNode, Literal};
    use crate::ident::{ClassId, Symbol, TyVar};

    fn class(name: &str) -> Ty {
        Ty::Class { id: ClassId(Symbol::new(name)), args: vec![] }
    }

    #[test]
    fn render_ty_uses_ruby_facing_names() {
        assert_eq!(render_ty(&Ty::Int), "Integer");
        assert_eq!(render_ty(&Ty::Str), "String");
        assert_eq!(render_ty(&Ty::Bool), "bool");
        assert_eq!(render_ty(&Ty::Nil), "nil");
        assert_eq!(render_ty(&Ty::Array { elem: Box::new(Ty::Int) }), "Array[Integer]");
        assert_eq!(
            render_ty(&Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) }),
            "Hash[Symbol, String]"
        );
        assert_eq!(render_ty(&class("Article")), "Article");
    }

    #[test]
    fn render_ty_collapses_nilable_union_to_question_mark() {
        let t = Ty::Union { variants: vec![class("Article"), Ty::Nil] };
        assert_eq!(render_ty(&t), "Article?");
        // Wider unions keep an explicit `nil` arm.
        let wide = Ty::Union { variants: vec![Ty::Int, Ty::Str, Ty::Nil] };
        assert_eq!(render_ty(&wide), "Integer | String | nil");
    }

    #[test]
    fn unresolved_variable_renders_untyped() {
        assert_eq!(render_ty(&Ty::Var { var: TyVar(7) }), "untyped");
        assert_eq!(render_ty(&Ty::Untyped), "untyped");
    }

    #[test]
    fn can_be_nil_only_for_provable_nil() {
        assert!(can_be_nil(&Ty::Nil));
        assert!(can_be_nil(&Ty::Union { variants: vec![class("Article"), Ty::Nil] }));
        assert!(!can_be_nil(&class("Article")));
        assert!(!can_be_nil(&Ty::Var { var: TyVar(0) }));
        assert!(!can_be_nil(&Ty::Untyped));
    }

    #[test]
    fn position_round_trips_through_byte_offsets() {
        let text = "ab\ncd\nef";
        for off in 0..=text.len() as u32 {
            let pos = offset_to_position(text, off);
            assert_eq!(position_to_offset(text, pos), off, "offset {off}");
        }
        assert_eq!(offset_to_position(text, 0), Position { line: 0, character: 0 });
        assert_eq!(offset_to_position(text, 3), Position { line: 1, character: 0 });
    }

    #[test]
    fn position_is_utf16_aware() {
        // "𝕏" is one scalar but two UTF-16 code units; "é" is one of each.
        let text = "é𝕏x";
        // byte offsets: é=0..2, 𝕏=2..6, x=6..7
        assert_eq!(offset_to_position(text, 2), Position { line: 0, character: 1 });
        assert_eq!(offset_to_position(text, 6), Position { line: 0, character: 3 });
        assert_eq!(position_to_offset(text, Position { line: 0, character: 3 }), 6);
    }

    #[test]
    fn position_clamps_past_end() {
        let text = "ab\n";
        assert_eq!(position_to_offset(text, Position { line: 9, character: 0 }), text.len() as u32);
        assert_eq!(
            position_to_offset(text, Position { line: 0, character: 99 }),
            2 // clamps to the newline at end of line 0
        );
    }

    /// End-to-end: ingest + analyze the real-blog fixture, then exercise
    /// the position queries against the genuinely-typed IR. This is the
    /// de-risk the issue cares about — querying never panics, even on
    /// every byte of every file, and real inferred types surface.
    #[test]
    fn query_real_blog_end_to_end() {
        use crate::analyze::Analyzer;
        use crate::ingest::ingest_app;
        use std::path::Path;

        let path = Path::new("fixtures/real-blog");
        let (ingest_result, _parse_diags) =
            crate::ingest::prism::scope(|| ingest_app(path));
        let mut app = ingest_result.expect("real-blog should ingest");
        Analyzer::new(&app).analyze(&mut app);

        // 1. Robustness: querying every byte of every source never panics,
        //    and offset<->position round-trips hold across real source.
        for (i, src) in app.sources.iter().enumerate() {
            let file = FileId(i as u32 + 1);
            let len = src.text.len() as u32;
            let mut off = 0u32;
            while off <= len {
                let _ = type_at(&app, file, off);
                let pos = offset_to_position(&src.text, off);
                // Round-trip only holds at char boundaries; offset_to_position
                // clamps to the start, so this is exact for boundary offsets.
                if src.text.is_char_boundary(off as usize) {
                    assert_eq!(
                        position_to_offset(&src.text, pos),
                        off,
                        "round-trip at {}:{off}",
                        src.path
                    );
                }
                off += 1;
            }
        }

        // 2. The pipeline surfaces real, concrete inferred types — find the
        //    first plain string literal and confirm it reads as `String`,
        //    non-nilable, located inside its own span.
        let mut checked_a_string = false;
        for root in root_bodies(&app) {
            let mut hit: Option<Span> = None;
            walk(root, &mut |e| {
                if hit.is_none()
                    && !e.span.is_synthetic()
                    && matches!(&*e.node, ExprNode::Lit { value: Literal::Str { .. } })
                {
                    hit = Some(e.span);
                }
            });
            if let Some(span) = hit {
                let probe = span.start + (span.len() / 2).max(1);
                let info = type_at(&app, span.file, probe)
                    .expect("a string literal should resolve to a typed node");
                assert_eq!(info.display, "String", "string literal at {span:?}");
                assert!(!info.nilable);
                checked_a_string = true;
                break;
            }
        }
        assert!(checked_a_string, "real-blog should contain a string literal");

        // 3. At least one position resolves to a *named class* type — proof
        //    the Rails-aware inference (not just literals) reaches the query
        //    layer.
        let mut saw_class = false;
        for (i, _src) in app.sources.iter().enumerate() {
            let file = FileId(i as u32 + 1);
            let len = source(&app, file).unwrap().text.len() as u32;
            let mut off = 0u32;
            while off < len && !saw_class {
                if let Some(info) = type_at(&app, file, off) {
                    if matches!(info.ty, Some(Ty::Class { .. })) {
                        saw_class = true;
                    }
                }
                off += 1;
            }
            if saw_class {
                break;
            }
        }
        assert!(saw_class, "real-blog inference should yield at least one class-typed position");
    }
}
