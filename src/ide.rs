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
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol, VarId};
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

/// Visit every node whose span overlaps the byte range `[start, end)` in
/// `file`, in pre-order. The range-scoped counterpart of
/// [`find_at_offset`], for consumers that decorate a viewport rather than
/// a point — inlay hints, semantic tokens. Synthetic-span nodes never
/// overlap a real file and are skipped.
pub fn nodes_in_range<'a>(
    app: &'a App,
    file: FileId,
    start: u32,
    end: u32,
    f: &mut dyn FnMut(&'a Expr),
) {
    for root in root_bodies(app) {
        walk(root, &mut |e| {
            if e.span.file == file && e.span.start < end && start < e.span.end {
                f(e);
            }
        });
    }
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

/// The app's typed bodies grouped by the class scope that owns them — one
/// inner vec per controller, model, view, and `db/seeds.rb`. Within a
/// scope: controller action bodies (plus the `Unknown` class-body DSL
/// exprs the analyzer types in its Phase-0 pass — `broadcasts_to ->(_a)
/// { "articles" }`, bare macro calls); model scope/method bodies (plus
/// `Unknown`); a view's body; the seeds expression.
///
/// The grouping is what lets [`references`] scope an instance-variable
/// lookup to a single class; [`root_bodies`] flattens it for whole-app
/// point queries.
fn scope_groups(app: &App) -> Vec<Vec<&Expr>> {
    let mut groups = Vec::new();
    for controller in &app.controllers {
        let mut group: Vec<&Expr> = controller.actions().map(|a| &a.body).collect();
        for item in &controller.body {
            if let ControllerBodyItem::Unknown { expr, .. } = item {
                group.push(expr);
            }
        }
        if !group.is_empty() {
            groups.push(group);
        }
    }
    for model in &app.models {
        let mut group: Vec<&Expr> = Vec::new();
        for scope in model.scopes() {
            group.push(&scope.body);
        }
        for method in model.methods() {
            group.push(&method.body);
        }
        for item in &model.body {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                group.push(expr);
            }
        }
        if !group.is_empty() {
            groups.push(group);
        }
    }
    for view in &app.views {
        groups.push(vec![&view.body]);
    }
    if let Some(seeds) = &app.seeds {
        groups.push(vec![seeds]);
    }
    groups
}

/// Every typed root body in the app, flattened from [`scope_groups`].
fn root_bodies(app: &App) -> Vec<&Expr> {
    scope_groups(app).into_iter().flatten().collect()
}

/// Pre-order subtree walk: visit `e`, then recurse into its children.
fn walk<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| walk(c, f));
}

// ── References (Rung 4) ──────────────────────────────────────────────
//
// A reverse def→use lookup over the two variable kinds the IR identifies
// exactly: locals carry a `VarId` binding id (unique within a body), and
// instance variables a name (resolved within a class). Method and
// constant references — which need type-aware dispatch resolution — are a
// later increment; this covers the precise, high-value cases.

/// One occurrence of a variable or method. `write` marks an assignment /
/// binding (variables only). `certain` marks a type-resolved match: it is
/// always true for locals/ivars (which are exact), and for a method call
/// distinguishes a receiver whose type resolves to the target class from a
/// same-named call whose receiver type couldn't be resolved (`false`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    pub span: Span,
    pub write: bool,
    pub certain: bool,
}

/// Which variable a position names.
enum VarRef {
    /// Local, by its exact binding id (unique within one body).
    Local(VarId),
    /// Instance variable, by name (resolved within its class).
    Ivar(Symbol),
}

/// All references to the variable or method at `offset` in `file`.
///
/// Variables resolve exactly and come back `certain`: locals by their
/// binding id (scoped to the enclosing body — a `VarId` is only unique
/// there), instance variables by name (scoped to the enclosing class, as a
/// Rails dev reads `@article`). Reads carry their real span; write targets
/// in `x = …` / `x += …` carry a span synthesized at the assigned name.
///
/// A method/attribute call resolves by the inferred **receiver type**:
/// every `recv.m` whose receiver type is the same class is `certain`, calls
/// on a different class are excluded, and same-named calls whose receiver
/// type couldn't be resolved come back `uncertain`. That type direction is
/// what lets `article.title` references exclude `comment.title` — precision
/// a name-based tool can't reach. Method spans point at the name token.
///
/// Empty when `offset` isn't on a resolvable variable or typed call.
/// Results are position-sorted and deduplicated.
pub fn references(app: &App, file: FileId, offset: u32) -> Vec<Reference> {
    if let Some(refs) = variable_references(app, file, offset) {
        return refs;
    }
    method_references(app, file, offset).unwrap_or_default()
}

fn variable_references(app: &App, file: FileId, offset: u32) -> Option<Vec<Reference>> {
    let (group, body) = locate(app, file, offset)?;
    let node = find_at_offset(app, file, offset)?;
    let var = var_at(node, offset)?;
    // Locals are body-scoped; instance variables span the whole class.
    let bodies: Vec<&Expr> = match var {
        VarRef::Local(_) => vec![body],
        VarRef::Ivar(_) => group,
    };
    let mut out = Vec::new();
    for &b in &bodies {
        collect_refs(b, &var, &mut out);
    }
    out.sort_by_key(|r| (r.span.file.0, r.span.start, r.span.end));
    out.dedup();
    Some(out)
}

/// References to the method/attribute named at `offset`, resolved by the
/// receiver's inferred type. `None` unless the cursor is on the name of an
/// explicit-receiver call whose receiver type resolves to a class — the
/// case where type direction is meaningful. (Implicit-`self` calls and
/// method *definitions* aren't handled yet; the latter needs a def-header
/// span the IR doesn't carry.)
fn method_references(app: &App, file: FileId, offset: u32) -> Option<Vec<Reference>> {
    let node = find_at_offset(app, file, offset)?;
    let ExprNode::Send { recv: Some(recv), method, .. } = &*node.node else {
        return None;
    };
    // The cursor must be on the method name, not the receiver — clicking the
    // receiver is a variable lookup (handled above). The receiver subtree
    // ends before the `.method` token.
    if offset < recv.span.end {
        return None;
    }
    let target = resolve_receiver_class(recv.ty.as_ref())?;

    let mut out = Vec::new();
    for body in root_bodies(app) {
        walk(body, &mut |e| {
            let ExprNode::Send { recv: Some(r), method: m, .. } = &*e.node else {
                return;
            };
            if m != method {
                return;
            }
            let certain = match resolve_receiver_class(r.ty.as_ref()) {
                Some(id) if id == target => true,
                Some(_) => return, // a different class — type rules it out
                None => false,     // receiver type unknown — name-only match
            };
            if let Some(span) = method_name_span(e, r, m, app) {
                out.push(Reference { span, write: false, certain });
            }
        });
    }
    out.sort_by_key(|r| (r.span.file.0, r.span.start, r.span.end));
    out.dedup();
    Some(out)
}

/// The class a receiver resolves to, if its type pins exactly one: a
/// `Class`, or a union of one class arm with `nil` (`Article?` → `Article`).
fn resolve_receiver_class(ty: Option<&Ty>) -> Option<ClassId> {
    match ty? {
        Ty::Class { id, .. } => Some(id.clone()),
        Ty::Union { variants } => {
            let mut found: Option<ClassId> = None;
            for v in variants {
                match v {
                    Ty::Nil => {}
                    Ty::Class { id, .. } if found.is_none() => found = Some(id.clone()),
                    // a second class, or a non-nil non-class arm → ambiguous
                    _ => return None,
                }
            }
            found
        }
        _ => None,
    }
}

/// The span of just the method-name token within a call. The IR carries no
/// selector span, so it's recovered from source: the first occurrence of
/// the name after the receiver (i.e. right after the `.`/`&.`). Falls back
/// to the whole call span when it can't be located (e.g. operator methods).
fn method_name_span(send: &Expr, recv: &Expr, method: &Symbol, app: &App) -> Option<Span> {
    let src = source(app, send.span.file)?;
    let from = recv.span.end as usize;
    let to = (send.span.end as usize).min(src.text.len());
    let name = method.as_str();
    if let Some(slice) = src.text.get(from..to) {
        if let Some(pos) = slice.find(name) {
            let start = (from + pos) as u32;
            return Some(Span { file: send.span.file, start, end: start + name.len() as u32 });
        }
    }
    Some(send.span)
}

/// The defining occurrence — the binding / earliest write — of the
/// variable at `offset`, if one exists in the searched scope. A
/// method-parameter local has reads but no in-body binding, so this is
/// `None` even though [`references`] still finds its uses.
pub fn definition(app: &App, file: FileId, offset: u32) -> Option<Span> {
    references(app, file, offset).into_iter().find(|r| r.write).map(|r| r.span)
}

/// The variable named at a position: a `Var`/`Ivar` read node, or — when
/// the cursor sits on the left of an assignment, whose target has no `Expr`
/// of its own — the assignment's target.
fn var_at(node: &Expr, offset: u32) -> Option<VarRef> {
    match &*node.node {
        ExprNode::Var { id, .. } => Some(VarRef::Local(*id)),
        ExprNode::Ivar { name } => Some(VarRef::Ivar(name.clone())),
        ExprNode::Assign { target, value } if offset < value.span.start => lvalue_var(target),
        ExprNode::OpAssign { target, value, .. } if offset < value.span.start => lvalue_var(target),
        _ => None,
    }
}

fn lvalue_var(lv: &LValue) -> Option<VarRef> {
    match lv {
        LValue::Var { id, .. } => Some(VarRef::Local(*id)),
        LValue::Ivar { name } => Some(VarRef::Ivar(name.clone())),
        _ => None,
    }
}

fn collect_refs(body: &Expr, var: &VarRef, out: &mut Vec<Reference>) {
    walk(body, &mut |e| {
        match (&*e.node, var) {
            (ExprNode::Var { id, .. }, VarRef::Local(want)) if id == want => {
                out.push(Reference { span: e.span, write: false, certain: true });
            }
            (ExprNode::Ivar { name }, VarRef::Ivar(want)) if name == want => {
                out.push(Reference { span: e.span, write: false, certain: true });
            }
            (ExprNode::Assign { target, .. }, _) => write_target(target, e.span, var, out),
            (ExprNode::OpAssign { target, .. }, _) => write_target(target, e.span, var, out),
            // Only the first target of a multi-assign sits at the statement
            // start, so only that one gets an accurate synthesized span.
            (ExprNode::MultiAssign { targets, .. }, _) => {
                if let Some(first) = targets.first() {
                    write_target(first, e.span, var, out);
                }
            }
            _ => {}
        }
    });
}

/// Push a write reference if `lv` names the variable, with a span
/// synthesized at the assigned name (a statement begins at its target).
fn write_target(lv: &LValue, stmt: Span, var: &VarRef, out: &mut Vec<Reference>) {
    let (matches, name_len) = match (lv, var) {
        (LValue::Var { id, name }, VarRef::Local(want)) => (id == want, name.as_str().len() as u32),
        // The `@` prefix is part of the source token but not the symbol name.
        (LValue::Ivar { name }, VarRef::Ivar(want)) => (name == want, name.as_str().len() as u32 + 1),
        _ => (false, 0),
    };
    if matches {
        out.push(Reference {
            span: Span { file: stmt.file, start: stmt.start, end: stmt.start + name_len },
            write: true,
            certain: true,
        });
    }
}

/// The scope group (a class's bodies) and the single body within it that
/// contains `offset`. The group scopes ivar lookups; the body scopes
/// locals (whose `VarId`s are only unique per body).
fn locate(app: &App, file: FileId, offset: u32) -> Option<(Vec<&Expr>, &Expr)> {
    for group in scope_groups(app) {
        if let Some(body) = group.iter().copied().find(|&b| subtree_contains(b, file, offset)) {
            return Some((group, body));
        }
    }
    None
}

fn subtree_contains(body: &Expr, file: FileId, offset: u32) -> bool {
    let mut found = false;
    walk(body, &mut |e| {
        if covers(e.span, file, offset) {
            found = true;
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::Analyzer;
    use crate::expr::Literal;
    use crate::ingest::ingest_app;
    use std::path::Path;

    fn real_blog() -> App {
        let (ir, _) = crate::ingest::prism::scope(|| ingest_app(Path::new("fixtures/real-blog")));
        let mut app = ir.expect("real-blog should ingest");
        Analyzer::new(&app).analyze(&mut app);
        app
    }
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

    #[test]
    fn references_finds_ivar_across_controller_actions() {
        let app = real_blog();
        let file = file_id(&app, "app/controllers/articles_controller.rb").expect("controller");
        let src = source(&app, file).unwrap();
        // `@article = …` (singular; `find("@article")` alone would match the
        // plural `@articles`). Landing on the assignment LHS also exercises
        // resolving a reference from a write site.
        let at = src.text.find("@article =").expect("@article assignment") as u32 + 1;

        let refs = references(&app, file, at);
        assert!(refs.len() >= 5, "expected many @article refs, got {}", refs.len());
        assert!(refs.iter().any(|r| r.write), "should include the @article assignment(s)");
        assert!(refs.iter().all(|r| r.span.file == file), "ivar scope is the one controller file");
        // The declaration resolves to a write site.
        assert!(definition(&app, file, at).is_some());
    }

    #[test]
    fn references_local_is_body_scoped() {
        let app = real_blog();
        let file = file_id(&app, "app/controllers/articles_controller.rb").unwrap();
        let src = source(&app, file).unwrap();
        // The `format` block param of a `respond_to do |format|` is read at
        // `format.html` / `format.json` within one action body.
        let at = src.text.find("format.html").expect("respond_to block") as u32 + 1;

        let refs = references(&app, file, at);
        assert!(refs.len() >= 2, "format used 2+ times in its body, got {}", refs.len());
    }

    #[test]
    fn references_method_is_type_directed() {
        let app = real_blog();
        let file = file_id(&app, "app/controllers/articles_controller.rb").unwrap();
        let src = source(&app, file).unwrap();
        // `@article.errors` — a method call on an Article-typed receiver,
        // appearing in more than one action.
        let dot = src.text.find(".errors").expect("an .errors call");
        let at = dot as u32 + 1; // inside the `errors` name token

        let refs = references(&app, file, at);
        assert!(refs.len() >= 2, "expected the @article.errors calls, got {}", refs.len());
        assert!(refs.iter().all(|r| r.certain), "Article-typed receivers are certain");
        // Spans point at just the method-name token, not the whole call.
        assert!(
            refs.iter().all(|r| r.span.len() == "errors".len() as u32),
            "method references should span only the name token"
        );
    }

    #[test]
    fn references_empty_off_a_variable() {
        let app = real_blog();
        let file = file_id(&app, "app/models/article.rb").unwrap();
        let src = source(&app, file).unwrap();
        // Inside the `"articles"` string literal — not a variable.
        let at = src.text.find("\"articles\"").unwrap() as u32 + 2;
        assert!(references(&app, file, at).is_empty());
    }

    /// Regression for the class-body DSL coverage gap: the
    /// `broadcasts_to ->(_article) { "articles" }` lambda in
    /// `app/models/article.rb` lives in a `ModelBodyItem::Unknown`, which
    /// the analyzer types but `root_bodies` once skipped. Its string
    /// literal must now resolve to `String`.
    #[test]
    fn type_at_reaches_class_body_dsl() {
        let app = real_blog();
        let file = file_id(&app, "app/models/article.rb").expect("article.rb is a source");
        let src = source(&app, file).unwrap();
        let quote = src.text.find("\"articles\"").expect("broadcasts_to string literal");
        // A byte inside the string literal (past the opening quote).
        let info = type_at(&app, file, quote as u32 + 2)
            .expect("class-body DSL string should now resolve to a typed node");
        assert_eq!(info.display, "String");
        assert!(!info.nilable);
    }

    /// End-to-end: ingest + analyze the real-blog fixture, then exercise
    /// the position queries against the genuinely-typed IR. This is the
    /// de-risk the issue cares about — querying never panics, even on
    /// every byte of every file, and real inferred types surface.
    #[test]
    fn query_real_blog_end_to_end() {
        let app = real_blog();

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
