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

use std::collections::{HashMap, HashSet};

use crate::analyze::ClassInfo;
use crate::app::App;
use crate::dialect::{AccessorKind, ControllerBodyItem, ModelBodyItem};
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
    /// Why the type reads the way it does, when a Rails reader would
    /// otherwise push back: a `belongs_to` reader is typed `T?` although
    /// the association is required, because the reader IS nil before
    /// assignment and on an orphaned row. The type stays sound; the
    /// note says so. `None` for everything else.
    pub note: Option<String>,
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
    let expr = find_at_offset(app, file, offset)?;
    let mut info = describe(expr);
    info.note = required_belongs_to_note(app, expr);
    Some(info)
}

/// `micropost.user` where `Micropost` declares `belongs_to :user` and
/// the association is not `optional:` — the reader types `User?`, and
/// this says why.
fn required_belongs_to_note(app: &App, expr: &Expr) -> Option<String> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let owner = match recv.ty.as_ref()? {
        Ty::Class { id, .. } => id,
        Ty::Union { variants } => variants.iter().find_map(|v| match v {
            Ty::Class { id, .. } => Some(id),
            _ => None,
        })?,
        _ => return None,
    };
    let model = app.models.iter().find(|m| &m.name == owner)?;
    let concern_items = crate::analyze::model_includes(model)
        .into_iter()
        .filter_map(|m| app.concern_model_items.get(&m))
        .flatten();
    let declared = model
        .associations()
        .chain(concern_items.filter_map(|item| match item {
            crate::dialect::ModelBodyItem::Association { assoc, .. } => Some(assoc),
            _ => None,
        }))
        .any(|a| {
            matches!(a, crate::dialect::Association::BelongsTo { name, optional: false, .. } if name == method)
        });
    declared.then(|| {
        "required `belongs_to` — `nil` only on an unsaved record or an orphaned row".to_string()
    })
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
        note: None,
    }
}

/// Whether a value of this type can be `nil`: the type is `nil`, or a
/// union with a `nil` arm (transitively). Unknown / untyped / gradual
/// positions return `false` — we only claim nilability we can prove.
/// Consumers answering the *question* "can this be nil?" should prefer
/// [`nil_verdict`], which distinguishes a proven "no" from "can't tell".
pub fn can_be_nil(ty: &Ty) -> bool {
    match ty {
        Ty::Nil => true,
        Ty::Union { variants } => variants.iter().any(can_be_nil),
        _ => false,
    }
}

/// Three-valued nilability: `Some(true)` when the type provably admits
/// nil, `Some(false)` when it provably cannot, `None` when the inference
/// has nothing to stand on — the position is untyped, an unresolved
/// inference variable, or a union with such an arm (an `untyped` arm
/// could be nil at runtime; claiming "cannot be nil" there would be an
/// overclaim). A union that *also* carries a proven `nil` arm stays
/// `Some(true)` — nil is possible regardless of what the unknown arm is.
pub fn nil_verdict(ty: Option<&Ty>) -> Option<bool> {
    fn unknown_arm(ty: &Ty) -> bool {
        match ty {
            Ty::Untyped | Ty::Var { .. } => true,
            Ty::Union { variants } => variants.iter().any(unknown_arm),
            _ => false,
        }
    }
    let ty = ty?;
    if can_be_nil(ty) {
        return Some(true);
    }
    if unknown_arm(ty) {
        return None;
    }
    Some(false)
}

// ── Member enumeration (completion substrate) ────────────────────────
//
// Dispatch resolves one name against a receiver's class; completion
// needs the inverse — every name the receiver responds to. The sources
// are the analyzer's class registry (the same table dispatch consults:
// schema columns, catalog-sourced AR surface, associations, scopes,
// user methods with inferred returns) re-classified against the App's
// model metadata so a completion item can say *what kind* of member it
// is (a column is a field, an association jumps to another model, a
// scope chains). Deliberately not covered: the built-in scalar surface
// (String/Array/Hash/Time methods), which lives in `send.rs` match
// arms rather than enumerable tables — commodity completions other
// tools already provide; the differentiated ones are the Rails ones.

/// What kind of member a completion item is, for editor `kind` mapping
/// and ranking. Classification is best-effort from model metadata;
/// registry entries with no richer story are `Method`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    /// A schema column (or `attr_accessor` virtual attribute).
    Column,
    /// An association reader/writer (`belongs_to`, `has_many`, …).
    Association,
    /// A named scope (class-side, chains as a relation).
    Scope,
    /// An attr_reader/attr_writer-shaped accessor.
    Accessor,
    /// Everything else callable.
    Method,
}

/// One name a receiver responds to, with its inferred type.
#[derive(Clone, Debug)]
pub struct Member {
    pub name: Symbol,
    pub kind: MemberKind,
    /// Return/attribute type when the registry knows one. `None` for
    /// registered-but-untyped entries.
    pub ty: Option<Ty>,
    /// Human-facing rendering of `ty` (RBS-flavoured, like hover).
    pub display: String,
}

/// Which side of the class the receiver is: `user.` completes instance
/// members, `User.` completes class members (scopes, finders). The
/// type system flattens both onto `Ty::Class { id }`, so the caller
/// decides syntactically — a constant receiver is the class object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberSide {
    Instance,
    Class,
}

/// Every member `class_id` responds to on `side`, walking `include`s
/// and the parent chain (nearest definition wins on a name collision,
/// mirroring dispatch). Output is name-sorted for stable presentation;
/// writer twins (`title=`) sort adjacent to their readers.
pub fn members_of(
    app: &App,
    registry: &HashMap<ClassId, ClassInfo>,
    class_id: &ClassId,
    side: MemberSide,
) -> Vec<Member> {
    let mut out: HashMap<Symbol, Member> = HashMap::new();
    // Own class first, then includes, then up the parent chain (BFS via
    // pop_front so includes are consulted before the parent, mirroring
    // dispatch) — insert-if-absent makes the nearest definition win.
    let mut queue: std::collections::VecDeque<&ClassId> = std::collections::VecDeque::new();
    queue.push_back(class_id);
    let mut visited: HashSet<&ClassId> = HashSet::new();
    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        let Some(cls) = registry.get(id) else { continue };

        // Model metadata for kind classification: association and scope
        // names, so registry entries that came from those declarations
        // present as what they are.
        let model = app.models.iter().find(|m| &m.name == id);
        let assoc_names: HashSet<&str> = model
            .map(|m| {
                m.associations()
                    .map(|a| match a {
                        crate::dialect::Association::BelongsTo { name, .. }
                        | crate::dialect::Association::HasMany { name, .. }
                        | crate::dialect::Association::HasOne { name, .. }
                        | crate::dialect::Association::HasAndBelongsToMany { name, .. } => {
                            name.as_str()
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let scope_names: HashSet<&str> = model
            .map(|m| m.scopes().map(|s| s.name.as_str()).collect())
            .unwrap_or_default();

        let mut add = |name: &Symbol, ty: Option<&Ty>, kind: MemberKind| {
            out.entry(name.clone()).or_insert_with(|| Member {
                name: name.clone(),
                kind,
                ty: ty.cloned(),
                display: ty.map(render_ty).unwrap_or_else(|| "untyped".to_string()),
            });
        };

        match side {
            MemberSide::Instance => {
                // Schema columns / attr_accessor state, reader + writer.
                for (name, ty) in &cls.attributes.fields {
                    add(name, Some(ty), MemberKind::Column);
                    add(&Symbol::from(format!("{}=", name.as_str())), Some(ty), MemberKind::Column);
                }
                for (name, ty) in &cls.instance_methods {
                    let base = name.as_str().strip_suffix('=').unwrap_or(name.as_str());
                    let kind = if assoc_names.contains(base) {
                        MemberKind::Association
                    } else {
                        match cls.instance_method_kinds.get(name) {
                            Some(AccessorKind::Method) | None => MemberKind::Method,
                            Some(_) => MemberKind::Accessor,
                        }
                    };
                    add(name, Some(ty), kind);
                }
            }
            MemberSide::Class => {
                for (name, ty) in &cls.class_methods {
                    let kind = if scope_names.contains(name.as_str()) {
                        MemberKind::Scope
                    } else {
                        MemberKind::Method
                    };
                    add(name, Some(ty), kind);
                }
            }
        }

        for inc in &cls.includes {
            queue.push_back(inc);
        }
        if let Some(parent) = &cls.parent {
            queue.push_back(parent);
        }
    }

    let mut members: Vec<Member> = out.into_values().collect();
    members.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    members
}

// ── Related files (Rails-aware navigation) ───────────────────────────
//
// RubyMine's "Related Symbol" jump, but driven by the inferred render
// graph and include edges instead of naming conventions: a controller
// relates to the views its actions *actually* feed (`App::view_feeders`),
// a view to the partials it *actually* renders (`App::render_edges`),
// a concern to the classes that include it. Convention appears only
// where no analyzed edge exists (controller ↔ model pairing).

/// What relates the file to the queried one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelatedKind {
    /// A view this controller's actions feed.
    View,
    /// A partial this view renders.
    Partial,
    /// A view that renders this partial.
    Renderer,
    /// A controller feeding this view.
    Controller,
    /// A concern this class includes.
    Concern,
    /// A class including this concern.
    Includer,
    /// The conventional model twin of this controller (or vice versa).
    Model,
}

#[derive(Clone, Debug)]
pub struct RelatedFile {
    pub kind: RelatedKind,
    /// Display name (view name, class name).
    pub label: String,
    /// Source path, as ingested (resolves via [`file_id`]).
    pub path: String,
}

/// Files related to `path` through analyzed edges. Empty when the file
/// isn't a controller/model/view/concern the analysis knows.
pub fn related_files(app: &App, path: &str) -> Vec<RelatedFile> {
    let Some(file) = file_id(app, path) else { return Vec::new() };
    let mut out: Vec<RelatedFile> = Vec::new();

    let src_path = |f: FileId| source(app, f).map(|s| s.path.clone());
    let view_file = |name: &Symbol| {
        app.views
            .iter()
            .find(|v| &v.name == name)
            .and_then(|v| first_expr_file(&v.body))
            .and_then(src_path)
    };
    let class_files: HashMap<ClassId, FileId> = class_file_index(app);
    let class_path = |id: &ClassId| class_files.get(id).copied().and_then(src_path);

    // Controller?
    if let Some(c) = app
        .controllers
        .iter()
        .find(|c| class_files.get(&c.name) == Some(&file))
    {
        for (view, feeders) in &app.view_feeders {
            if feeders.contains(&c.name) && !view.as_str().starts_with("layouts/") {
                if let Some(p) = view_file(view) {
                    out.push(RelatedFile {
                        kind: RelatedKind::View,
                        label: view.as_str().to_string(),
                        path: p,
                    });
                }
            }
        }
        for inc in crate::analyze::controller_includes(c) {
            if let Some(p) = class_path(&inc) {
                out.push(RelatedFile {
                    kind: RelatedKind::Concern,
                    label: inc.0.as_str().to_string(),
                    path: p,
                });
            }
        }
        // Conventional model twin: StatusesController → Status.
        let base = c.name.0.as_str().rsplit("::").next().unwrap_or("");
        if let Some(plural) = base.strip_suffix("Controller") {
            let singular = crate::naming::singularize_camelize(plural);
            let id = ClassId(Symbol::from(singular));
            if app.models.iter().any(|m| m.name == id) {
                if let Some(p) = class_path(&id) {
                    out.push(RelatedFile {
                        kind: RelatedKind::Model,
                        label: id.0.as_str().to_string(),
                        path: p,
                    });
                }
            }
        }
    }

    // Model?
    if let Some(m) = app.models.iter().find(|m| class_files.get(&m.name) == Some(&file)) {
        for inc in crate::analyze::model_includes(m) {
            if let Some(p) = class_path(&inc) {
                out.push(RelatedFile {
                    kind: RelatedKind::Concern,
                    label: inc.0.as_str().to_string(),
                    path: p,
                });
            }
        }
        let plural = crate::naming::pluralize_snake(m.name.0.as_str());
        for c in &app.controllers {
            let base = c.name.0.as_str().rsplit("::").next().unwrap_or("");
            if base
                .strip_suffix("Controller")
                .is_some_and(|b| crate::naming::pluralize_snake(b) == plural)
            {
                if let Some(p) = class_path(&c.name) {
                    out.push(RelatedFile {
                        kind: RelatedKind::Controller,
                        label: c.name.0.as_str().to_string(),
                        path: p,
                    });
                }
            }
        }
    }

    // View?
    if let Some(v) = app.views.iter().find(|v| first_expr_file(&v.body) == Some(file)) {
        if let Some(feeders) = app.view_feeders.get(&v.name) {
            for f in feeders {
                if let Some(p) = class_path(f) {
                    out.push(RelatedFile {
                        kind: RelatedKind::Controller,
                        label: f.0.as_str().to_string(),
                        path: p,
                    });
                }
            }
        }
        if let Some(partials) = app.render_edges.get(&v.name) {
            for partial in partials {
                if let Some(p) = view_file(partial) {
                    out.push(RelatedFile {
                        kind: RelatedKind::Partial,
                        label: partial.as_str().to_string(),
                        path: p,
                    });
                }
            }
        }
        for (renderer, partials) in &app.render_edges {
            if partials.contains(&v.name) {
                if let Some(p) = view_file(renderer) {
                    out.push(RelatedFile {
                        kind: RelatedKind::Renderer,
                        label: renderer.as_str().to_string(),
                        path: p,
                    });
                }
            }
        }
    }

    // Concern module? (a library-class module defined in this file)
    for lc in &app.library_classes {
        if !lc.is_module || class_files.get(&lc.name) != Some(&file) {
            continue;
        }
        for c in &app.controllers {
            if crate::analyze::controller_includes(c).contains(&lc.name) {
                if let Some(p) = class_path(&c.name) {
                    out.push(RelatedFile {
                        kind: RelatedKind::Includer,
                        label: c.name.0.as_str().to_string(),
                        path: p,
                    });
                }
            }
        }
        for m in &app.models {
            if crate::analyze::model_includes(m).contains(&lc.name) {
                if let Some(p) = class_path(&m.name) {
                    out.push(RelatedFile {
                        kind: RelatedKind::Includer,
                        label: m.name.0.as_str().to_string(),
                        path: p,
                    });
                }
            }
        }
    }

    out.sort_by(|a, b| (a.kind as u8, a.label.as_str()).cmp(&(b.kind as u8, b.label.as_str())));
    out.dedup_by(|a, b| a.kind == b.kind && a.label == b.label);
    out
}

/// Defining file per class: models carry a span; controllers, library
/// classes, and everything else resolve through their first
/// real-spanned body expression.
pub fn class_file_index(app: &App) -> HashMap<ClassId, FileId> {
    let mut out = HashMap::new();
    for m in &app.models {
        if !m.span.is_synthetic() {
            out.insert(m.name.clone(), m.span.file);
        }
    }
    for c in &app.controllers {
        if let Some(f) = c.actions().find_map(|a| first_expr_file(&a.body)) {
            out.insert(c.name.clone(), f);
        }
    }
    for lc in &app.library_classes {
        if let Some(f) = lc.methods.iter().find_map(|m| first_expr_file(&m.body)) {
            out.entry(lc.name.clone()).or_insert(f);
        }
    }
    out
}

/// First non-synthetic file a subtree touches.
fn first_expr_file(e: &Expr) -> Option<FileId> {
    if !e.span.is_synthetic() {
        return Some(e.span.file);
    }
    let mut found = None;
    e.node.for_each_child(&mut |c| {
        if found.is_none() {
            found = first_expr_file(c);
        }
    });
    found
}

// ── Completion (transport-free core) ─────────────────────────────────
//
// The full completion pipeline, shared by every skin: the LSP handler
// maps candidates to `lsp_types::CompletionItem`, the wasm/browser
// skin to Monaco items. Answers from a *last-good* analysis plus the
// *current* buffer text: the receiver expression almost always
// predates the keystroke that triggered the request (`user` existed
// before its `.` was typed), so typing it against the stale snapshot
// is correct in practice, and the client filters against the live
// prefix. Three shapes:
//   `recv.…`              → members of the receiver's inferred class
//                           (instance side for values, class side for
//                           constants — scopes, finders)
//   `@…`                  → ivars observed in this file, with types
//   `find_by(…`/`where(…` → the receiver model's columns as kwargs

/// What a completion candidate is, for editor `kind` mapping and
/// ranking. A superset of [`MemberKind`] (adds the non-member shapes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    Column,
    Association,
    Scope,
    Accessor,
    Method,
    Ivar,
    Kwarg,
}

/// One completion candidate, protocol-free. `detail` is the rendered
/// type (hover-style); `rank` orders the Rails-shaped members above
/// the generic framework surface ('0' leads); `insert_text` is `None`
/// when the label inserts verbatim.
#[derive(Clone, Debug)]
pub struct CompletionCandidate {
    pub label: String,
    pub kind: CandidateKind,
    pub detail: String,
    pub rank: char,
    pub insert_text: Option<String>,
}

/// Complete at `cursor` (byte offset into `text`, the *current* buffer
/// for `path`), against a last-good analysis. `None` when the position
/// isn't a completion context.
pub fn complete_at(
    app: &App,
    registry: &HashMap<ClassId, ClassInfo>,
    path: &str,
    text: &str,
    cursor: usize,
) -> Option<Vec<CompletionCandidate>> {
    let cursor = cursor.min(text.len());
    let start = word_start(text, cursor);
    let word = &text[start..cursor];
    if word.starts_with('@') {
        return ivar_candidates(app, path);
    }
    let before = *text.as_bytes().get(start.checked_sub(1)?)?;
    match before {
        b'.' => member_candidates(app, registry, path, text, start - 1),
        b'(' | b',' | b' ' => kwarg_candidates(app, registry, path, text, start),
        _ => None,
    }
}

fn member_candidates(
    app: &App,
    registry: &HashMap<ClassId, ClassInfo>,
    path: &str,
    text: &str,
    dot: usize,
) -> Option<Vec<CompletionCandidate>> {
    let (class_id, side) = receiver_class(app, registry, path, text, dot)?;
    let members = members_of(app, registry, &class_id, side);
    if members.is_empty() {
        return None;
    }
    Some(members.iter().map(member_candidate).collect())
}

fn ivar_candidates(app: &App, path: &str) -> Option<Vec<CompletionCandidate>> {
    let seen = file_ivars(app, path);
    if seen.is_empty() {
        return None;
    }
    let mut items: Vec<CompletionCandidate> = seen
        .into_iter()
        .map(|(name, ty)| {
            let display = ty.as_ref().map(render_ty).unwrap_or_else(|| "untyped".to_string());
            CompletionCandidate {
                label: format!("@{}", name.as_str()),
                kind: CandidateKind::Ivar,
                detail: display,
                rank: '0',
                insert_text: None,
            }
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    Some(items)
}

fn kwarg_candidates(
    app: &App,
    registry: &HashMap<ClassId, ClassInfo>,
    path: &str,
    text: &str,
    upto: usize,
) -> Option<Vec<CompletionCandidate>> {
    let dot = kwarg_call_dot(text, upto)?;
    // Whichever side the receiver resolved on (`User.find_by(` is the
    // class object), the kwargs are the model's columns — enumerate
    // the instance side of the resolved class.
    let (class_id, _) = receiver_class(app, registry, path, text, dot)?;
    let members = members_of(app, registry, &class_id, MemberSide::Instance);
    let items: Vec<CompletionCandidate> = members
        .iter()
        .filter(|m| m.kind == MemberKind::Column && !m.name.as_str().ends_with('='))
        .map(|m| CompletionCandidate {
            label: format!("{}:", m.name.as_str()),
            kind: CandidateKind::Kwarg,
            detail: m.display.clone(),
            rank: '0',
            insert_text: Some(format!("{}: ", m.name.as_str())),
        })
        .collect();
    if items.is_empty() { None } else { Some(items) }
}

fn member_candidate(m: &Member) -> CompletionCandidate {
    let kind = match m.kind {
        MemberKind::Column => CandidateKind::Column,
        MemberKind::Association => CandidateKind::Association,
        MemberKind::Scope => CandidateKind::Scope,
        MemberKind::Accessor => CandidateKind::Accessor,
        MemberKind::Method => CandidateKind::Method,
    };
    let rank = match m.kind {
        MemberKind::Column | MemberKind::Association | MemberKind::Scope => '0',
        MemberKind::Accessor => '1',
        MemberKind::Method => '2',
    };
    CompletionCandidate {
        label: m.name.as_str().to_string(),
        kind,
        detail: m.display.clone(),
        rank,
        insert_text: None,
    }
}

/// Bytes that extend the word being completed: Ruby identifier chars,
/// the ivar sigil, `?`/`!` method suffixes, and `:` so qualified
/// constants (`Admin::Report`) scan as one token.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'@' | b'?' | b'!' | b':')
}

/// Start offset of the word containing/preceding `cursor`.
pub fn word_start(text: &str, cursor: usize) -> usize {
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
/// (instance side: columns, associations). Falls back textually for
/// receivers on lines the stale snapshot hasn't seen: a constant
/// resolves by registry name, an ivar by the type the same ivar has
/// elsewhere in this file.
pub fn receiver_class(
    app: &App,
    registry: &HashMap<ClassId, ClassInfo>,
    path: &str,
    text: &str,
    dot: usize,
) -> Option<(ClassId, MemberSide)> {
    if dot == 0 {
        return None;
    }
    let pos = offset_to_position(text, (dot - 1) as u32);
    if let Some(info) = type_at_position(app, path, pos) {
        if let Some(id) = info.ty.as_ref().and_then(root_class) {
            let side = if info.node_kind == "Const" {
                MemberSide::Class
            } else {
                MemberSide::Instance
            };
            return Some((id, side));
        }
    }
    let start = word_start(text, dot);
    let token = &text[start..dot];
    if token.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        let id = ClassId(Symbol::from(token));
        if registry.contains_key(&id) {
            return Some((id, MemberSide::Class));
        }
    }
    if let Some(name) = token.strip_prefix('@') {
        let ivars = file_ivars(app, path);
        let ty = ivars.get(&Symbol::from(name)).and_then(|t| t.as_ref())?;
        return root_class(ty).map(|id| (id, MemberSide::Instance));
    }
    None
}

/// The root class of a receiver type: `Article` itself, or the class
/// arm of a nilable union (`Article?` completes as `Article` — the
/// user gets members while the nil-safety diagnostics separately flag
/// the unguarded call).
pub fn root_class(ty: &Ty) -> Option<ClassId> {
    match ty {
        Ty::Class { id, .. } => Some(id.clone()),
        Ty::Union { variants } => variants.iter().find_map(root_class),
        _ => None,
    }
}

/// Every ivar observed in `path`'s analyzed exprs, with the first
/// resolved type seen for each (reads and assignment values both
/// contribute). The substrate for `@` completion and for typing an
/// ivar receiver on a line the stale snapshot hasn't seen.
pub fn file_ivars(app: &App, path: &str) -> HashMap<Symbol, Option<Ty>> {
    let mut seen: HashMap<Symbol, Option<Ty>> = HashMap::new();
    let Some(file) = file_id(app, path) else { return seen };
    nodes_in_range(app, file, 0, u32::MAX, &mut |e| {
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

/// For a cursor inside a call's argument list, the offset of the `.`
/// linking the receiver to a kwarg-completable method (`find_by`,
/// `find_by!`, `where`). Scans left for the innermost unbalanced `(`,
/// then requires `recv.method(` immediately before it.
pub fn kwarg_call_dot(text: &str, upto: usize) -> Option<usize> {
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
        Ty::Time => "Time".to_string(),
        Ty::Nil => "nil".to_string(),
        // Consumer-facing projection of the analysis-time relation
        // type: an unmaterialized query over `of`. `Relation[Story]`
        // is what a Rails developer expects to read on hover for
        // `Story.recent` — unlike emit type positions (which must
        // report Unsupported), showing the relation here is the
        // honest rendering.
        Ty::Relation { of } => format!("Relation[{of}]"),
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
///
/// An offset landing INSIDE a character clamps back to that character's
/// start, which is what the round-trip contract already assumed. Slicing
/// there instead panics, and the offsets reaching here are not all
/// boundary-aligned: they come from spans over real source, where one
/// non-ASCII character (an em dash in a template comment is enough) puts
/// interior byte positions in range. An LSP query must not crash the
/// server.
pub fn offset_to_position(text: &str, offset: u32) -> Position {
    let mut offset = (offset as usize).min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
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
    // Concerns, mailers, helpers, lib/: their method bodies are typed
    // in the library-class pass, so hover, definition and references
    // answer inside them too (the guide's `Product::Notifications` and
    // `ProductMailer` were blank).
    for lc in &app.library_classes {
        let group: Vec<&Expr> = lc.methods.iter().map(|m| &m.body).collect();
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
    // Locals are body-scoped; instance variables span the whole class —
    // and, for a template, the class is the controller that feeds it.
    let bodies: Vec<&Expr> = match &var {
        VarRef::Local(_) => vec![body],
        VarRef::Ivar(_) => {
            if let Some(view) = app.views.iter().find(|v| std::ptr::eq(&v.body, body)) {
                // Rails' ivar channel: the writes ran in the feeding
                // action (and its filters) before the template read
                // them. Path bodies first, in execution order, so the
                // first write IS the definition; then this template and
                // the others the same actions render.
                let mut bodies = view_ivar_bodies(app, &view.name);
                bodies.push(body);
                let mut out = Vec::new();
                for &b in &bodies {
                    collect_refs(b, &var, &mut out);
                }
                return Some(dedup_keep_order(out));
            }
            let mut g = group;
            // The other direction: an ivar in a controller is read by
            // the templates its actions feed.
            if let Some(c) = app.controllers.iter().find(|c| c.actions().any(|a| std::ptr::eq(&a.body, body))) {
                for view in &app.views {
                    if app.view_feeders.get(&view.name).is_some_and(|f| f.contains(&c.name)) {
                        g.push(&view.body);
                    }
                }
            }
            g
        }
    };
    let mut out = Vec::new();
    for &b in &bodies {
        collect_refs(b, &var, &mut out);
    }
    out.sort_by_key(|r| (r.span.file.0, r.span.start, r.span.end));
    out.dedup();
    Some(out)
}

/// First occurrence wins; order is the caller's.
fn dedup_keep_order(refs: Vec<Reference>) -> Vec<Reference> {
    let mut seen = std::collections::HashSet::new();
    refs.into_iter().filter(|r| seen.insert((r.span.file.0, r.span.start, r.span.end))).collect()
}

/// The bodies an ivar read in `view` can resolve against, in the order
/// they run: for every action that renders the view (directly, or
/// through the renderers of a partial), its applying filters' bodies
/// then the action body; then the sibling templates those actions
/// render. Composed from the same edges the trace and the ivar-type
/// seeding use — `view_feeders`, `view_name_for_action`, the resolved
/// filter chain, `render_edges`.
fn view_ivar_bodies<'a>(app: &'a App, view: &Symbol) -> Vec<&'a Expr> {
    // Views whose render tree reaches `view`: itself, plus renderers
    // transitively (a partial's ivars come from whoever renders it).
    let mut targets: Vec<Symbol> = vec![view.clone()];
    let mut qi = 0;
    while qi < targets.len() {
        let t = targets[qi].clone();
        qi += 1;
        for (renderer, partials) in &app.render_edges {
            if partials.contains(&t) && !targets.contains(renderer) {
                targets.push(renderer.clone());
            }
        }
    }
    let mut bodies: Vec<&'a Expr> = Vec::new();
    let mut siblings: Vec<&'a Expr> = Vec::new();
    let Some(feeders) = app.view_feeders.get(view) else { return bodies };
    for cid in feeders {
        let Some(controller) = find_controller(app, cid) else { continue };
        let resolution = app.controller_resolutions.get(cid);
        for action in controller.actions() {
            let Some(rendered) = crate::analyze::view_name_for_action(cid, action) else { continue };
            if !targets.contains(&rendered) {
                continue;
            }
            if let Some(res) = resolution {
                for rf in &res.filter_chain {
                    if !matches!(rf.filter.kind, crate::dialect::FilterKind::Before)
                        || !crate::analyze::before_filter_applies(&rf.filter, &action.name)
                    {
                        continue;
                    }
                    let block_body = rf.filter.block.as_ref().map(|call| match &*call.node {
                        ExprNode::Send { block: Some(b), .. } => match &*b.node {
                            ExprNode::Lambda { body, .. } => body,
                            _ => b,
                        },
                        _ => call,
                    });
                    if let Some(b) = block_body
                        .or_else(|| method_body(app, &rf.defined_in, &rf.filter.target))
                        .or_else(|| method_body(app, cid, &rf.filter.target))
                    {
                        bodies.push(b);
                    }
                }
            }
            bodies.push(&action.body);
            // The templates this action renders, and their partials.
            let mut tree: Vec<Symbol> = vec![rendered.clone()];
            let mut ti = 0;
            while ti < tree.len() {
                let t = tree[ti].clone();
                ti += 1;
                if let Some(ps) = app.render_edges.get(&t) {
                    for p in ps {
                        if !tree.contains(p) {
                            tree.push(p.clone());
                        }
                    }
                }
            }
            for v in &app.views {
                if &v.name != view && tree.contains(&v.name) {
                    siblings.push(&v.body);
                }
            }
        }
    }
    bodies.extend(siblings);
    bodies
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

// ── Traceroute (#63): the full request flow as one query ─────────────
//
// Composes edges the analyzer already persisted — `RouteTable` (via
// `flatten_routes`), `App::controller_resolutions` (chained filters
// with provenance + effective layout), action `RenderTarget`s, and
// `render_edges` — into the ordered causal chain one request
// traverses. One query, two skins: the MCP `traceroute` tool
// serializes it, the /ide/ trace panel renders it. Everything here is
// composition; the resolution work happened in `run_typing_passes`.

/// A complete trace for one `Controller#action` entry point.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    /// Display line: `GET /articles/:id → ArticlesController#show`
    /// (just `Controller#action` when no route matched — actions
    /// reachable only via `render`/mailers still trace).
    pub route: String,
    pub controller: String,
    pub action: String,
    /// Hops in request order: route → filters (chain order, gated-out
    /// entries included with `applies: false`) → action → view (or
    /// non-template response) → layout.
    pub hops: Vec<TraceHop>,
}

/// One hop of the chain. Filter payload is broken out as
/// [`FilterHop`] — it carries most of the fields and the skins treat
/// it as a unit.
#[derive(Clone, Debug, PartialEq)]
pub enum TraceHop {
    /// The matched route entry: verb, path pattern, and the params the
    /// path binds.
    Route { method: String, path: String, params: Vec<String> },
    Filter(FilterHop),
    /// The action body. `formats` comes from the explicit
    /// `RenderTarget` or, for convention renders, from the ingested
    /// templates matching the view name.
    Action {
        name: String,
        controller: String,
        file: Option<String>,
        line: Option<u32>,
        formats: Vec<String>,
        /// Ivars the action body itself assigns (filter contributions
        /// live on their own hops), rendered `("@article", "Article")`,
        /// sorted by name.
        assigns: Vec<(String, String)>,
        effects: Vec<String>,
        n_plus_one: Vec<PreloadFinding>,
    },
    /// Non-template terminal — redirect / JSON / head.
    Response { detail: String },
    /// The template the action feeds, plus every partial it renders
    /// transitively (deduped, discovery order). `n_plus_one` covers the
    /// template *and* its partials — the finding's own `file` says
    /// which.
    View {
        name: String,
        file: Option<String>,
        partials: Vec<String>,
        n_plus_one: Vec<PreloadFinding>,
    },
    /// The effective layout wrapping the response.
    Layout { name: String, file: Option<String>, n_plus_one: Vec<PreloadFinding> },
}

/// One missing-preload finding (#64's static N+1 pass) attached to the
/// trace hop whose body or template contains the access site — #63
/// phase 5's hop annotation. The finding record is shared with the
/// diagnostics skin (same pass, same message), so the trace and the
/// report never disagree.
#[derive(Clone, Debug, PartialEq)]
pub struct PreloadFinding {
    /// The association the iteration reads without a preload.
    pub association: String,
    /// Access site (a partial's file when the read is inside one).
    pub file: Option<String>,
    pub line: Option<u32>,
    /// Query origin as `path:line` — where the `.includes` belongs.
    pub query_site: Option<String>,
    /// The full diagnostic message (both sites + the one-line fix).
    pub message: String,
}

/// One filter hop, in chain position. `applies` reflects this action:
/// `only:`/`except:` gating plus `skip_before_action` entries that
/// target it. Gated-out hops are kept (the panel's "N filters don't
/// run for this action" needs them); `skipped_by` distinguishes an
/// explicit skip (names the class that declared it) from plain
/// gating (`None`).
#[derive(Clone, Debug, PartialEq)]
pub struct FilterHop {
    pub name: String,
    /// `"before"` / `"around"` / `"after"`.
    pub filter_kind: &'static str,
    /// Class or concern module that declared the filter.
    pub defined_in: String,
    /// Chain segment that carried it in (the includer for concern
    /// filters). Equals `defined_in` for directly-declared filters.
    pub included_via: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// Whether the target method's body is in the IR. `false` marks a
    /// dispatch boundary — gem/framework-defined (Devise's
    /// `authenticate_user!`) or lost to an ingest gap; the gap footer
    /// ([`trace_gap_report`]) attributes which.
    pub resolved: bool,
    /// Guard as written: `if: :account_required?` /
    /// `unless: :limited_federation_mode?` (both, comma-joined, when
    /// both present). Carried verbatim; see `decided` for the case the
    /// static chain CAN evaluate.
    pub condition: Option<String>,
    /// When the guard is a predicate over the request's verb
    /// (`request.get? || request.head?`) and the trace has a matched
    /// route, the guard is decided for that verb: `(verb, runs)`.
    /// `runs == false` folds into `applies`. None when there is no
    /// guard, no route, or the predicate reads anything but the verb —
    /// an undecidable guard leaves the hop applying, as before.
    pub decided: Option<(String, bool)>,
    pub only: Vec<String>,
    pub except: Vec<String>,
    pub applies: bool,
    pub skipped_by: Option<String>,
    /// Typed ivars the target assigns, `("@account", "Account")`,
    /// sorted by name.
    pub assigns: Vec<(String, String)>,
    pub effects: Vec<String>,
    /// Missing-preload findings whose access site is inside this
    /// filter's target body (annotated only when the hop applies —
    /// a gated-out filter does no work on this request).
    pub n_plus_one: Vec<PreloadFinding>,
}

/// Trace the request flow for `query`: either `Controller#action`
/// (`StatusesController#show`) or a route (`GET /articles/:id`, verb
/// optional — the path is matched against the route table's patterns).
/// `None` when the query names no known controller/action/route.
pub fn traceroute(app: &App, query: &str) -> Option<Trace> {
    let q = query.trim();
    let routes = crate::lower::routes::flatten_routes(app);

    // Resolve the entry point to (controller, action, matched route).
    let (controller_id, action_name, matched) = if let Some((c, a)) = q.split_once('#') {
        let cid = ClassId(Symbol::from(c.trim()));
        let act = Symbol::from(a.trim());
        let m = routes.iter().find(|r| r.controller == cid && r.action == act);
        (cid, act, m)
    } else {
        let (verb, path) = match q.split_once(' ') {
            Some((v, p)) if parse_verb(v).is_some() => (parse_verb(v), p.trim()),
            _ => (None, q),
        };
        let m = routes.iter().find(|r| {
            r.path == path && verb.as_ref().is_none_or(|v| &r.method == v)
        })?;
        (m.controller.clone(), m.action.clone(), Some(m))
    };

    // The controller must at least be known; a missing resolution
    // (analyze not run) degrades to an empty chain rather than a miss.
    let controller = find_controller(app, &controller_id)?;
    let resolution = app.controller_resolutions.get(&controller_id).cloned().unwrap_or_default();

    // #63 phase 5: #64's missing-preload findings annotate the hop
    // whose body/template contains the access site. The pass is
    // app-wide (the controller→view ivar channel needs every feeder),
    // so run it once per trace and attach by span containment.
    let preload_diags = crate::analyze::missing_preload_report(app).0;
    // The bodies this request runs — applying filters and the action —
    // so a template finding is attached only when ITS query is on this
    // path (see `preloads_in_files`).
    let mut path_bodies: Vec<&Expr> = Vec::new();

    let route_line = match matched {
        Some(r) => format!(
            "{} {} → {}#{}",
            verb_str(&r.method),
            r.path,
            controller_id.0.as_str(),
            action_name.as_str()
        ),
        None => format!("{}#{}", controller_id.0.as_str(), action_name.as_str()),
    };

    let mut hops: Vec<TraceHop> = Vec::new();
    if let Some(r) = matched {
        hops.push(TraceHop::Route {
            method: verb_str(&r.method).to_string(),
            path: r.path.clone(),
            params: r.path_params.clone(),
        });
    }

    // Applicable `skip_before_action` declarations, target → declarer.
    // Rails' skip gating is itself `only:`/`except:`-scoped, so apply
    // per-action here rather than at persist time.
    let mut skips: HashMap<Symbol, ClassId> = HashMap::new();
    for rf in &resolution.filter_chain {
        if matches!(rf.filter.kind, crate::dialect::FilterKind::Skip)
            && crate::analyze::before_filter_applies(&rf.filter, &action_name)
        {
            skips.insert(rf.filter.target.clone(), rf.defined_in.clone());
        }
    }

    // Lookup surface for filter targets: a filter declared in one
    // class routinely names a method defined in another (a child
    // declares `before_action :require_login`, ApplicationController
    // defines it; a concern's `included do` names a method another
    // concern provides). Search the declaring class first, then every
    // class on the chain, then the ancestor walk with each ancestor's
    // own includes.
    let target_search: Vec<ClassId> = {
        let mut out: Vec<ClassId> = Vec::new();
        let push = |id: &ClassId, out: &mut Vec<ClassId>| {
            if !out.contains(id) {
                out.push(id.clone());
            }
        };
        for rf in &resolution.filter_chain {
            push(&rf.defined_in, &mut out);
            push(&rf.included_via, &mut out);
        }
        let mut cur = Some(controller);
        let mut guard = 0;
        while let Some(c) = cur {
            push(&c.name, &mut out);
            for inc in crate::analyze::controller_includes(c) {
                push(&inc, &mut out);
            }
            guard += 1;
            if guard > 32 {
                break;
            }
            cur = parent_of(app, c);
        }
        // Close over concern-includes-concern (SignatureAuthentication
        // includes SignatureVerification, which defines the method a
        // controller's filter names) — mirror analyze's mixed_in
        // closure.
        let mut qi = 0;
        while qi < out.len() {
            let m = out[qi].clone();
            qi += 1;
            let Some(lc) = app.library_classes.iter().find(|lc| lc.name == m) else {
                continue;
            };
            for inc in &lc.includes {
                push(inc, &mut out);
            }
        }
        out
    };

    for rf in &resolution.filter_chain {
        use crate::dialect::FilterKind;
        let kind = match rf.filter.kind {
            FilterKind::Before => "before",
            FilterKind::Around => "around",
            FilterKind::After => "after",
            FilterKind::Skip => continue, // annotates the hop it removes
        };
        let gated_in = crate::analyze::before_filter_applies(&rf.filter, &action_name);
        // `skip_before_action` removes before callbacks only.
        let skipped_by = (matches!(rf.filter.kind, FilterKind::Before) && gated_in)
            .then(|| skips.get(&rf.filter.target).cloned())
            .flatten();
        // A block-form filter has no method to find: its body is the
        // block on the call the chain entry carries, and it resolves by
        // construction (the code is right there). Named under its own
        // source text so the panel reads `before_action { Current.request
        // = request }` rather than the chain's sentinel.
        let block_body: Option<&Expr> = rf.filter.block.as_ref().map(|call| match &*call.node {
            ExprNode::Send { block: Some(b), .. } => match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            },
            _ => call,
        });
        let target_body = block_body.or_else(|| {
            method_body(app, &rf.defined_in, &rf.filter.target).or_else(|| {
                target_search
                    .iter()
                    .find_map(|c| method_body(app, c, &rf.filter.target))
            })
        });
        let resolved = target_body.is_some();
        let (file, line) = match target_body {
            Some(body) => expr_location(app, body),
            None => (None, None),
        };
        let name = match block_body {
            Some(body) => format!("{} {{ {} }}", kind_word(&rf.filter.kind), block_summary(app, body)),
            None => rf.filter.target.as_str().to_string(),
        };
        let condition = match (&rf.filter.if_cond, &rf.filter.unless_cond) {
            (None, None) => None,
            (i, u) => Some(
                [
                    i.as_ref().map(|s| format!("if: :{}", s.as_str())),
                    u.as_ref().map(|s| format!("unless: :{}", s.as_str())),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", "),
            ),
        };
        // The route's verb decides verb-only guards. campfire's
        // `reject_banned_ip unless: :safe_request?` with `safe_request?
        // = request.get? || request.head?` does not run on GET; the
        // chain used to list it as applying, condition shown as text,
        // and an agent reading the trace said it ran.
        let verb = matched.map(|r| verb_str(&r.method)).filter(|v| *v != "ANY");
        let decided = verb.and_then(|v| {
            let runs = guard_verdict(app, &rf.filter, &rf.defined_in, &target_search, v)?;
            Some((v.to_string(), runs))
        });
        let applies = gated_in && skipped_by.is_none() && !matches!(decided, Some((_, false)));
        let n_plus_one = match (applies, target_body) {
            (true, Some(body)) => {
                path_bodies.push(body);
                preloads_in_body(app, &preload_diags, body)
            }
            _ => Vec::new(),
        };
        hops.push(TraceHop::Filter(FilterHop {
            name,
            filter_kind: kind,
            defined_in: rf.defined_in.0.as_str().to_string(),
            included_via: rf.included_via.0.as_str().to_string(),
            file,
            line,
            resolved,
            condition,
            decided,
            only: rf.filter.only.iter().map(|s| s.as_str().to_string()).collect(),
            except: rf.filter.except.iter().map(|s| s.as_str().to_string()).collect(),
            applies,
            skipped_by: skipped_by.map(|c| c.0.as_str().to_string()),
            assigns: render_assigns(&rf.assigns),
            effects: rf.effects.effects.iter().map(render_effect).collect(),
            n_plus_one,
        }));
    }

    // The action: defined on the queried controller or inherited.
    let action_def = find_action(app, controller, &action_name);
    // Ingest classifies an action as `Template` or `Inferred` only; a
    // `create` that redirects is `Inferred` → the convention template
    // `rooms/create`, which the app does not carry. When the body says
    // how it responds instead, the chain ends in that response rather
    // than a phantom view.
    let response_detail = action_def.and_then(|(_, a)| response_terminal(&a.body));
    let view_name = action_def
        .and_then(|(_, a)| crate::analyze::view_name_for_action(&controller_id, a))
        .filter(|v| response_detail.is_none() || app.views.iter().any(|w| &w.name == v));
    {
        let (file, line, effects, assigns) = match action_def {
            Some((_, a)) => {
                let (f, l) = expr_location(app, &a.body);
                let mut ivars: HashMap<Symbol, Ty> = HashMap::new();
                crate::analyze::extract_ivar_assignments(&a.body, &mut ivars);
                (
                    f,
                    l,
                    a.effects.effects.iter().map(render_effect).collect(),
                    render_assigns(&ivars),
                )
            }
            // Routed but bodiless (template-only action): Rails still
            // renders the convention template.
            None => (None, None, Vec::new(), Vec::new()),
        };
        let formats: Vec<String> = match action_def.map(|(_, a)| &a.renders) {
            Some(crate::dialect::RenderTarget::Template { formats, .. })
                if !formats.is_empty() =>
            {
                formats.iter().map(|s| s.as_str().to_string()).collect()
            }
            _ => match &view_name {
                Some(v) => {
                    let mut f: Vec<String> = app
                        .views
                        .iter()
                        .filter(|w| &w.name == v)
                        .map(|w| w.format.as_str().to_string())
                        .collect();
                    f.sort();
                    f.dedup();
                    f
                }
                None => Vec::new(),
            },
        };
        let n_plus_one = match action_def {
            Some((_, a)) => {
                let bodies = with_helper_bodies(app, &target_search, &a.body);
                path_bodies.extend(bodies.iter().copied());
                bodies.iter().flat_map(|b| preloads_in_body(app, &preload_diags, b)).collect()
            }
            None => Vec::new(),
        };
        hops.push(TraceHop::Action {
            name: action_name.as_str().to_string(),
            controller: controller_id.0.as_str().to_string(),
            file,
            line,
            formats,
            assigns,
            effects,
            n_plus_one,
        });
    }

    // The response: a template (with its transitive partials) or a
    // non-template terminal.
    match &view_name {
        Some(v) => {
            let mut partials: Vec<String> = Vec::new();
            let mut queue: Vec<Symbol> = vec![v.clone()];
            let mut seen: HashSet<Symbol> = queue.iter().cloned().collect();
            while let Some(next) = queue.pop() {
                for p in app.render_edges.get(&next).into_iter().flatten() {
                    if seen.insert(p.clone()) {
                        partials.push(p.as_str().to_string());
                        queue.push(p.clone());
                    }
                }
            }
            // Findings in the template or any of its partials annotate
            // the view hop; `seen` holds the view plus every partial.
            let view_files: Vec<String> =
                seen.iter().filter_map(|s| view_location(app, s)).collect();
            hops.push(TraceHop::View {
                name: v.as_str().to_string(),
                file: view_location(app, v),
                partials,
                n_plus_one: preloads_in_files(app, &preload_diags, &view_files, &path_bodies),
            });
            if let Some(layout) = &resolution.layout {
                let file = view_location(app, layout);
                let n_plus_one = match &file {
                    Some(f) => preloads_in_files(
                        app,
                        &preload_diags,
                        std::slice::from_ref(f),
                        &path_bodies,
                    ),
                    None => Vec::new(),
                };
                hops.push(TraceHop::Layout {
                    name: layout.as_str().to_string(),
                    file,
                    n_plus_one,
                });
            }
        }
        None => {
            if let Some((_, a)) = action_def {
                use crate::dialect::RenderTarget;
                let detail = match &a.renders {
                    RenderTarget::Redirect { .. } => "redirect".to_string(),
                    RenderTarget::Json { .. } => "render json".to_string(),
                    RenderTarget::Head { status } => format!("head {status}"),
                    _ => response_detail.clone().unwrap_or_else(|| "no template".to_string()),
                };
                hops.push(TraceHop::Response { detail });
            }
        }
    }

    Some(Trace {
        route: route_line,
        controller: controller_id.0.as_str().to_string(),
        action: action_name.as_str().to_string(),
        hops,
    })
}


/// One traceable entry point, for pickers: every concrete route plus
/// every controller action whose convention template exists (real apps
/// route through DSL our routes ingest doesn't fully model — Mastodon's
/// `/@:username` — so route entries alone under-list what traces).
/// `query` is the canonical `Controller#action` form [`traceroute`]
/// accepts; `label` is the human line (route form when known).
#[derive(Clone, Debug, PartialEq)]
pub struct TraceTarget {
    pub label: String,
    pub query: String,
    pub controller: String,
}

/// Enumerate everything [`traceroute`] can trace, deduped by query,
/// routes first (in table order) then unrouted view-rendering actions.
pub fn trace_targets(app: &App) -> Vec<TraceTarget> {
    let mut out: Vec<TraceTarget> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in crate::lower::routes::flatten_routes(app) {
        let query = format!("{}#{}", r.controller.0.as_str(), r.action.as_str());
        if !seen.insert(query.clone()) {
            continue;
        }
        out.push(TraceTarget {
            label: format!("{} {} → {query}", verb_str(&r.method), r.path),
            query,
            controller: r.controller.0.as_str().to_string(),
        });
    }
    let view_names: HashSet<&Symbol> = app.views.iter().map(|v| &v.name).collect();
    for c in &app.controllers {
        for a in c.actions() {
            let Some(view) = crate::analyze::view_name_for_action(&c.name, a) else {
                continue;
            };
            if !view_names.contains(&view) {
                continue; // private helpers and non-rendering methods
            }
            let query = format!("{}#{}", c.name.0.as_str(), a.name.as_str());
            if !seen.insert(query.clone()) {
                continue;
            }
            out.push(TraceTarget {
                label: query.clone(),
                query,
                controller: c.name.0.as_str().to_string(),
            });
        }
    }
    out
}

fn parse_verb(s: &str) -> Option<crate::dialect::HttpMethod> {
    use crate::dialect::HttpMethod::*;
    Some(match s.to_ascii_uppercase().as_str() {
        "GET" => Get,
        "POST" => Post,
        "PUT" => Put,
        "PATCH" => Patch,
        "DELETE" => Delete,
        "HEAD" => Head,
        "OPTIONS" => Options,
        _ => return None,
    })
}

fn verb_str(m: &crate::dialect::HttpMethod) -> &'static str {
    use crate::dialect::HttpMethod::*;
    match m {
        Get => "GET",
        Post => "POST",
        Put => "PUT",
        Patch => "PATCH",
        Delete => "DELETE",
        Head => "HEAD",
        Options => "OPTIONS",
        Any => "ANY",
    }
}

fn find_controller<'a>(app: &'a App, id: &ClassId) -> Option<&'a crate::dialect::Controller> {
    app.controllers.iter().find(|c| &c.name == id)
}

/// The action's defining controller, walking the parent chain
/// (nearest-first) for inherited actions. Single-segment parents are
/// qualified against the child's namespace, mirroring analyze's
/// lexical resolution.
fn find_action<'a>(
    app: &'a App,
    controller: &'a crate::dialect::Controller,
    action: &Symbol,
) -> Option<(&'a ClassId, &'a crate::dialect::Action)> {
    let mut current = controller;
    let mut guard = 0;
    loop {
        if let Some(a) = current.actions().find(|a| &a.name == action) {
            return Some((&current.name, a));
        }
        guard += 1;
        if guard > 32 {
            return None;
        }
        current = parent_of(app, current)?;
    }
}

/// The parent controller, resolving single-segment parents against the
/// child's namespace (mirrors analyze's lexical resolution).
fn parent_of<'a>(
    app: &'a App,
    controller: &'a crate::dialect::Controller,
) -> Option<&'a crate::dialect::Controller> {
    let parent = controller.parent.as_ref()?;
    find_controller(app, parent).or_else(|| {
        if parent.0.as_str().contains("::") {
            return None;
        }
        let mut segs: Vec<&str> = controller.name.0.as_str().split("::").collect();
        segs.pop();
        while !segs.is_empty() {
            let candidate =
                ClassId(Symbol::from(format!("{}::{}", segs.join("::"), parent.0.as_str())));
            if let Some(c) = find_controller(app, &candidate) {
                return Some(c);
            }
            segs.pop();
        }
        None
    })
}

/// The IR body of `class_id#method`, if ingested — controllers keep
/// every method def as an `Action` item; concern/library modules as
/// `MethodDef`s. `None` is the dispatch-boundary signal the gap
/// footer attributes.
fn method_body<'a>(app: &'a App, class_id: &ClassId, method: &Symbol) -> Option<&'a Expr> {
    if let Some(c) = find_controller(app, class_id) {
        if let Some(a) = c.actions().find(|a| &a.name == method) {
            return Some(&a.body);
        }
    }
    if let Some(lc) = app.library_classes.iter().find(|lc| &lc.name == class_id) {
        if let Some(m) = lc.methods.iter().find(|m| &m.name == method) {
            return Some(&m.body);
        }
    }
    None
}

/// How an action body responds without a convention template, from
/// the calls it makes: `redirect_to`/`redirect_back`, `head`, `render
/// json:`, and explicit template renders (`render :new`). Every
/// distinct terminal in source order, ` · `-joined — a scaffold
/// `create` reads `redirect · render show · render new · render json`.
/// `None` when the body has no such call. The caller applies this only
/// when the convention template is absent: an action whose explicit
/// render ingest already resolved to one template keeps its view chain.
fn response_terminal(body: &Expr) -> Option<String> {
    use crate::expr::Literal;
    let mut details: Vec<String> = Vec::new();
    let mut add = |d: String| {
        if !details.contains(&d) {
            details.push(d);
        }
    };
    walk(body, &mut |e| {
        if let ExprNode::Send { recv: None, method, args, .. } = &*e.node {
            match method.as_str() {
                "redirect_to" | "redirect_back" | "redirect_back_or_to" => add("redirect".to_string()),
                "head" => {
                    let status = args.first().and_then(|a| match &*a.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                        ExprNode::Lit { value: Literal::Int { value } } => Some(value.to_string()),
                        _ => None,
                    });
                    add(match status {
                        Some(s) => format!("head {s}"),
                        None => "head".to_string(),
                    });
                }
                "render" => {
                    let json = args.iter().any(|a| match &*a.node {
                        ExprNode::Hash { entries, .. } => entries.iter().any(|(k, _)| {
                            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "json")
                        }),
                        _ => false,
                    });
                    if json {
                        add("render json".to_string());
                    } else if let Some(name) = crate::ingest::controller::render_template_name(args) {
                        add(format!("render {}", name.as_str()));
                    }
                }
                _ => {}
            }
        }
    });
    if details.is_empty() {
        None
    } else {
        Some(details.join(" · "))
    }
}

/// `body` plus the bodies of the chain's methods it calls by implicit
/// self, transitively — a `create` that delegates its iteration to a
/// private `broadcast_create_room` still owns that N+1, and the trace
/// must say so on the action hop. `chain` is the controller, its
/// ancestors and their concerns; a call that resolves nowhere in it
/// (a helper from a gem, a model method) contributes nothing.
fn with_helper_bodies<'a>(app: &'a App, chain: &[ClassId], body: &'a Expr) -> Vec<&'a Expr> {
    let mut out: Vec<&'a Expr> = vec![body];
    let mut seen: HashSet<Symbol> = HashSet::new();
    let mut i = 0;
    while i < out.len() {
        let mut calls: Vec<Symbol> = Vec::new();
        walk(out[i], &mut |e| {
            if let ExprNode::Send { recv: None, method, .. } = &*e.node {
                calls.push(method.clone());
            }
        });
        for m in calls {
            if !seen.insert(m.clone()) {
                continue;
            }
            if let Some(b) = chain.iter().find_map(|c| method_body(app, c, &m)) {
                out.push(b);
            }
        }
        i += 1;
    }
    out
}

/// Missing-preload findings whose access site is inside `body`.
fn preloads_in_body(
    app: &App,
    diags: &[crate::diagnostic::Diagnostic],
    body: &Expr,
) -> Vec<PreloadFinding> {
    diags
        .iter()
        .filter(|d| subtree_contains(body, d.span.file, d.span.start))
        .map(|d| preload_finding(app, d))
        .collect()
}

/// Missing-preload findings whose access site lands in one of `files`
/// (template hops match by file — one template per file) AND whose
/// query is on this request's path: in one of `path_bodies` (an
/// applying filter or the action), in one of the same templates (a
/// query written in the view), or in a model (a chain method the path
/// may reach — not attributable to one trace, so kept).
///
/// The access site alone over-attached: the Rails tutorial's
/// `_micropost` partial is rendered by `users#show` (whose query has no
/// preload — the finding) AND by the home feed (whose query preloads
/// correctly), and the home trace wore the badge for users#show's
/// query.
fn preloads_in_files(
    app: &App,
    diags: &[crate::diagnostic::Diagnostic],
    files: &[String],
    path_bodies: &[&Expr],
) -> Vec<PreloadFinding> {
    diags
        .iter()
        .filter(|d| source(app, d.span.file).is_some_and(|s| files.iter().any(|f| f == &s.path)))
        .filter(|d| {
            let crate::diagnostic::DiagnosticKind::MissingPreload { query_span, .. } = &d.kind
            else {
                return false;
            };
            let in_path = path_bodies
                .iter()
                .any(|b| subtree_contains(b, query_span.file, query_span.start));
            let in_files_or_model = source(app, query_span.file).is_some_and(|s| {
                files.iter().any(|f| f == &s.path) || s.path.contains("app/models/")
            });
            in_path || in_files_or_model
        })
        .map(|d| preload_finding(app, d))
        .collect()
}

fn preload_finding(app: &App, d: &crate::diagnostic::Diagnostic) -> PreloadFinding {
    let (association, query_site) = match &d.kind {
        crate::diagnostic::DiagnosticKind::MissingPreload { association, query_span } => {
            (association.as_str().to_string(), span_site(app, *query_span))
        }
        _ => (String::new(), None),
    };
    let (file, line) = match source(app, d.span.file) {
        Some(src) => (Some(src.path.clone()), Some(src.line_col(d.span.start).0)),
        None => (None, None),
    };
    PreloadFinding { association, file, line, query_site, message: d.message.clone() }
}

/// `path:line` for a non-synthetic span.
fn span_site(app: &App, span: Span) -> Option<String> {
    if span.is_synthetic() {
        return None;
    }
    let src = source(app, span.file)?;
    Some(format!("{}:{}", src.path, src.line_col(span.start).0))
}

/// Decide a filter's `if:` / `unless:` guard for a request verb, when
/// the guard is a predicate over nothing but the verb. Symbol guards
/// resolve to their method body (the filter's own class first, then the
/// chain); lambda guards evaluate their body directly. Both guards
/// present ⇒ both must decide. None = no guard, or not decidable.
fn guard_verdict(
    app: &App,
    filter: &crate::dialect::Filter,
    defined_in: &ClassId,
    search: &[ClassId],
    verb: &str,
) -> Option<bool> {
    let body_of = |sym: &Symbol| -> Option<&Expr> {
        method_body(app, defined_in, sym).or_else(|| search.iter().find_map(|c| method_body(app, c, sym)))
    };
    let decide = |sym: &Option<Symbol>, expr: &Option<Expr>| -> Option<Option<bool>> {
        match (expr, sym) {
            (Some(e), _) => Some(verb_predicate(e, verb)),
            (None, Some(s)) => Some(body_of(s).and_then(|b| verb_predicate(b, verb))),
            (None, None) => None,
        }
    };
    let if_v = decide(&filter.if_cond, &filter.if_cond_expr);
    let unless_v = decide(&filter.unless_cond, &filter.unless_cond_expr);
    if if_v.is_none() && unless_v.is_none() {
        return None;
    }
    // An undecidable guard is None inside Some: no verdict at all.
    let if_ok = match if_v {
        Some(v) => v?,
        None => true,
    };
    let unless_hit = match unless_v {
        Some(v) => v?,
        None => false,
    };
    Some(if_ok && !unless_hit)
}

/// Evaluate a verb predicate: `request.get?` / `.head?` / `.post?` /
/// `.put?` / `.patch?` / `.delete?` / `.options?`, combined with `&&`,
/// `||`, `!`, possibly as a one-statement method body. Anything else —
/// a param read, a model call, a second statement — is not a verb
/// predicate and yields None.
fn verb_predicate(e: &Expr, verb: &str) -> Option<bool> {
    const VERBS: &[&str] = &["get?", "head?", "post?", "put?", "patch?", "delete?", "options?"];
    match &*e.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => verb_predicate(&exprs[0], verb),
        ExprNode::BoolOp { op, left, right, .. } => {
            let l = verb_predicate(left, verb)?;
            let r = verb_predicate(right, verb)?;
            Some(match op {
                crate::expr::BoolOpKind::And => l && r,
                crate::expr::BoolOpKind::Or => l || r,
            })
        }
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "!" && args.is_empty() =>
        {
            verb_predicate(r, verb).map(|v| !v)
        }
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if args.is_empty() && VERBS.contains(&method.as_str()) =>
        {
            let ExprNode::Send { recv: None, method: rm, args: ra, block: None, .. } = &*r.node
            else {
                return None;
            };
            if rm.as_str() != "request" || !ra.is_empty() {
                return None;
            }
            Some(method.as_str().trim_end_matches('?').eq_ignore_ascii_case(verb))
        }
        _ => None,
    }
}

/// `before_action` / `around_action` / `after_action`, for naming a
/// block-form hop by its call.
fn kind_word(kind: &crate::dialect::FilterKind) -> &'static str {
    use crate::dialect::FilterKind;
    match kind {
        FilterKind::Before => "before_action",
        FilterKind::Around => "around_action",
        FilterKind::After => "after_action",
        FilterKind::Skip => "skip_before_action",
    }
}

/// The block body's first source line, whitespace-collapsed and
/// clipped, for a block-form hop's label. Falls back to `…` when the
/// span is synthetic.
fn block_summary(app: &App, body: &Expr) -> String {
    let Some(span) = first_expr_span(body) else { return "…".to_string() };
    let Some(src) = source(app, span.file) else { return "…".to_string() };
    let text = &src.text[span.start as usize..(span.end as usize).min(src.text.len())];
    let first = text.lines().next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
    let more = text.lines().nth(1).is_some();
    let mut out: String = first.chars().take(60).collect();
    if first.chars().count() > 60 || more {
        out.push('…');
    }
    out
}

fn expr_location(app: &App, body: &Expr) -> (Option<String>, Option<u32>) {
    let Some(span) = first_expr_span(body) else { return (None, None) };
    let Some(src) = source(app, span.file) else { return (None, None) };
    (Some(src.path.clone()), Some(src.line_col(span.start).0))
}

/// First non-synthetic span in a subtree (the sibling of
/// [`first_expr_file`], for consumers that need the offset too).
fn first_expr_span(e: &Expr) -> Option<Span> {
    if !e.span.is_synthetic() {
        return Some(e.span);
    }
    let mut found = None;
    e.node.for_each_child(&mut |c| {
        if found.is_none() {
            found = first_expr_span(c);
        }
    });
    found
}

fn view_location(app: &App, name: &Symbol) -> Option<String> {
    app.views
        .iter()
        .find(|v| &v.name == name)
        .and_then(|v| first_expr_file(&v.body))
        .and_then(|f| source(app, f).map(|s| s.path.clone()))
}

fn render_assigns(assigns: &HashMap<Symbol, Ty>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = assigns
        .iter()
        .map(|(k, v)| (format!("@{}", k.as_str()), render_ty(v)))
        .collect();
    out.sort();
    out
}

fn render_effect(e: &crate::effect::Effect) -> String {
    use crate::effect::Effect::*;
    match e {
        Io => "Io".to_string(),
        DbRead { table } => format!("DbRead({})", table.0.as_str()),
        DbWrite { table } => format!("DbWrite({})", table.0.as_str()),
        Time => "Time".to_string(),
        Random => "Random".to_string(),
        Raises { class } => format!("Raises({})", class.0.as_str()),
        Net { host } => match host {
            Some(h) => format!("Net({h})"),
            None => "Net".to_string(),
        },
        Log => "Log".to_string(),
        Var { .. } => "?".to_string(),
    }
}

// ── Trace gap footer (#63): priced, attributed, pre-filled ───────────
//
// The honesty layer over a [`Trace`]: which hops the analysis could
// not see through, split by whose problem each one is. Two kinds,
// deliberately kept visually separable by consumers:
//
//   - `UntypedBoundary` — user-actionable: a dispatch boundary an RBS
//     sidecar pins (gem/framework methods like Devise's
//     `authenticate_user!`, or app methods whose types went soft).
//     When the analyzer holds an inferred signature, it rides along
//     pre-filled (`candidate_rbs`) — accepting it is one file write.
//   - `IngestGap` — tool coverage, not the user's code: the boundary
//     traces to a construct our ingest recorded as unsupported.
//
// An empty report with all hops resolved is itself the product claim
// ("trace complete — all N hops resolved"): the skins render it
// affirmatively so "nothing to report" is distinguishable from
// "couldn't look".

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceGapKind {
    /// User-actionable: an RBS signature closes it.
    UntypedBoundary,
    /// Ours: an ingest gap swallowed the definition. Never phrased as
    /// a user ask.
    IngestGap,
}

#[derive(Clone, Debug)]
pub struct TraceGap {
    pub kind: TraceGapKind,
    /// What went soft: `Class#method` for boundaries, a source path
    /// for ingest gaps.
    pub boundary: String,
    /// One human sentence (the footer line).
    pub detail: String,
    /// How many of this trace's applicable hops the gap blocks — the
    /// price tag that sorts the footer.
    pub blocked_hops: usize,
    /// Pre-filled inferred signature (RBS `def` line) when the
    /// analyzer knows one — the accept action. `None` when nothing
    /// honest can be offered (an unresolved gem method the analyzer
    /// never saw).
    pub candidate_rbs: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TraceGapReport {
    /// Leverage-sorted: user-actionable boundaries first (by hops
    /// blocked), then tool-coverage entries.
    pub gaps: Vec<TraceGap>,
    /// Hops this request runs — the route, applying filters, the
    /// action, the view and the layout — and how many of them resolved
    /// (a body the analysis saw; a template or route it located).
    /// Gated-out filters are shown struck through and are not hops
    /// here, so a 7-row panel with three skipped filters reads 4/4,
    /// not the "all 1 hops resolved" the body-only count produced. A
    /// template the app does not carry is not counted either way.
    pub resolved_hops: usize,
    pub total_hops: usize,
}

impl TraceGapReport {
    /// The positive completeness claim: every applicable hop resolved
    /// and nothing in the footer.
    pub fn complete(&self) -> bool {
        self.gaps.is_empty() && self.resolved_hops == self.total_hops
    }
}

/// Build the gap footer for `trace`. `ingest_gaps` is the survey
/// collector's output for this app (empty in strict-mode runs — the
/// same contract as [`crate::analyze::attribution`]); `analyzer`, when
/// provided, supplies inferred candidate signatures for the
/// pre-filled accept action.
pub fn trace_gap_report(
    app: &App,
    trace: &Trace,
    ingest_gaps: &[crate::ingest::IngestError],
    analyzer: Option<&crate::analyze::Analyzer>,
) -> TraceGapReport {
    use crate::ingest::IngestError;
    // Gap file → recorded messages (deduped). IngestError paths carry
    // the same ingest-root prefix as `App::sources`, so exact match
    // with a suffix fallback (mirrors `file_id`) is enough.
    let mut gap_files: Vec<(String, Vec<String>)> = Vec::new();
    for e in ingest_gaps {
        let file = match e {
            IngestError::Parse { file, .. } | IngestError::Unsupported { file, .. } => file,
            IngestError::Io(_) => continue,
        };
        if file.is_empty() {
            continue; // no source to attribute to
        }
        // Bucketed message (pointer-bearing node dumps truncated), so
        // footer lines group and dedupe the way the survey report does.
        let message = crate::ingest::survey::bucket_key(e);
        match gap_files.iter_mut().find(|(f, _)| f == file) {
            Some((_, msgs)) => {
                if !msgs.contains(&message) {
                    msgs.push(message);
                }
            }
            None => gap_files.push((file.clone(), vec![message])),
        }
    }
    // Suffix match only at a path-component boundary — a bare
    // `ends_with` lets short/empty paths swallow everything.
    let paths_match = |a: &str, b: &str| {
        if a.is_empty() || b.is_empty() {
            return false;
        }
        if a == b {
            return true;
        }
        let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
        long.ends_with(short) && long.as_bytes()[long.len() - short.len() - 1] == b'/'
    };
    let gap_messages_for = |path: &str| -> Option<&Vec<String>> {
        gap_files.iter().find(|(f, _)| paths_match(f, path)).map(|(_, m)| m)
    };
    let class_files = class_file_index(app);
    let class_path_of = |name: &str| -> Option<String> {
        class_files
            .get(&ClassId(Symbol::from(name)))
            .and_then(|f| source(app, *f).map(|s| s.path.clone()))
    };

    let mut resolved_hops = 0usize;
    let mut total_hops = 0usize;
    // (defined_in, method) → hops blocked, insertion-ordered.
    let mut boundaries: Vec<((String, String), usize)> = Vec::new();
    // Files applicable hops run through, for the ingest-gap sweep.
    let mut hop_files: Vec<(String, usize)> = Vec::new();
    let touch_file = |files: &mut Vec<(String, usize)>, f: &Option<String>| {
        let Some(f) = f else { return };
        match files.iter_mut().find(|(p, _)| p == f) {
            Some((_, n)) => *n += 1,
            None => files.push((f.clone(), 1)),
        }
    };

    for hop in &trace.hops {
        match hop {
            TraceHop::Filter(f) if f.applies => {
                total_hops += 1;
                touch_file(&mut hop_files, &f.file);
                if f.resolved {
                    resolved_hops += 1;
                } else {
                    let key = (f.defined_in.clone(), f.name.clone());
                    match boundaries.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, n)) => *n += 1,
                        None => boundaries.push((key, 1)),
                    }
                }
            }
            TraceHop::Action { file, .. } => {
                // A bodiless routed action (template-only) is
                // legitimate Rails, not a boundary.
                total_hops += 1;
                resolved_hops += 1;
                touch_file(&mut hop_files, file);
            }
            // The route, the template and the layout are hops too. A
            // located template counts as a resolved hop; one the app
            // does not carry (a JSON action, a fixture without views)
            // is not a boundary and is left out of the count, as before.
            TraceHop::Route { .. } => {
                total_hops += 1;
                resolved_hops += 1;
            }
            TraceHop::View { file, .. } | TraceHop::Layout { file, .. } => {
                if file.is_some() {
                    total_hops += 1;
                    resolved_hops += 1;
                }
                touch_file(&mut hop_files, file);
            }
            _ => {}
        }
    }

    let mut user_gaps: Vec<TraceGap> = Vec::new();
    let mut tool_gaps: Vec<TraceGap> = Vec::new();

    for ((class, method), blocked) in boundaries {
        let boundary = format!("{class}#{method}");
        // Attribution: if the defining class's file recorded an ingest
        // gap, the definition likely exists but didn't survive ingest
        // — ours, not a user ask.
        let class_gap = class_path_of(&class).and_then(|p| {
            gap_messages_for(&p).map(|msgs| (p, msgs.first().cloned().unwrap_or_default()))
        });
        if let Some((path, msg)) = class_gap {
            tool_gaps.push(TraceGap {
                kind: TraceGapKind::IngestGap,
                boundary,
                detail: format!(
                    "target lost to an ingest gap in {path} ({msg}) — tool coverage, not your code"
                ),
                blocked_hops: blocked,
                candidate_rbs: None,
            });
            continue;
        }
        let candidate = analyzer.and_then(|a| {
            candidate_signature(app, a, &ClassId(Symbol::from(class.as_str())), &Symbol::from(method.as_str()))
        });
        let detail = match &candidate {
            Some(_) => format!(
                "{boundary} — body not in the IR; inferred signature below, accept: write sig/"
            ),
            None => format!(
                "{boundary} — gem/framework-defined; an RBS sidecar for it would let the chain type through"
            ),
        };
        user_gaps.push(TraceGap {
            kind: TraceGapKind::UntypedBoundary,
            boundary,
            detail,
            blocked_hops: blocked,
            candidate_rbs: candidate,
        });
    }

    // Ingest gaps on files this trace's hops run through: coverage
    // notes even when the hop itself resolved (the placeholder may
    // have swallowed a sibling method the chain depends on).
    for (path, hops) in &hop_files {
        let Some(msgs) = gap_messages_for(path) else { continue };
        for msg in msgs {
            tool_gaps.push(TraceGap {
                kind: TraceGapKind::IngestGap,
                boundary: path.clone(),
                detail: format!("{msg} — tool coverage, not your code"),
                blocked_hops: *hops,
                candidate_rbs: None,
            });
        }
    }

    user_gaps.sort_by(|a, b| b.blocked_hops.cmp(&a.blocked_hops));
    tool_gaps.sort_by(|a, b| b.blocked_hops.cmp(&a.blocked_hops));
    let mut gaps = user_gaps;
    gaps.extend(tool_gaps);
    TraceGapReport { gaps, resolved_hops, total_hops }
}

/// The inferred candidate signature for `class_id#method`, printed as
/// an RBS `def` line — the pre-fill behind the gap footer's accept
/// action. Sources, in order: a full `Ty::Fn` already in the dispatch
/// registry (an RBS overlay or an extraction-shaped entry) prints
/// directly; a bare inferred return type is combined with the IR
/// def's param names and the call-site-unified param types. `None`
/// when the registry knows nothing non-`untyped` — fabricating an
/// arity would burn the trust the accept loop depends on.
pub fn candidate_signature(
    app: &App,
    analyzer: &crate::analyze::Analyzer,
    class_id: &ClassId,
    method: &Symbol,
) -> Option<String> {
    let info = analyzer.class_registry().get(class_id)?;
    let known = info
        .instance_methods
        .get(method)
        .or_else(|| info.class_methods.get(method))?;
    if matches!(known, Ty::Fn { .. }) {
        return crate::rbs::print_method_signature(method.as_str(), known);
    }
    if known.is_unknown() {
        return None; // a `-> untyped` candidate prices nothing
    }
    let param_names = method_def_param_names(app, class_id, method)?;
    let inferred = analyzer.inferred_param_types(class_id, method);
    let params: Vec<crate::ty::Param> = param_names
        .iter()
        .enumerate()
        .map(|(i, name)| crate::ty::Param {
            name: name.clone(),
            ty: inferred.and_then(|ts| ts.get(i)).cloned().unwrap_or(Ty::Untyped),
            kind: crate::ty::ParamKind::Required,
        })
        .collect();
    let fn_ty = Ty::Fn {
        params,
        block: None,
        ret: Box::new(known.clone()),
        effects: crate::effect::EffectSet::pure(),
    };
    crate::rbs::print_method_signature(method.as_str(), &fn_ty)
}

/// Declared parameter names of `class_id#method` from the IR def —
/// controllers (Action.params rows) then library/concern modules
/// (MethodDef params). `None` when the def isn't in the IR.
fn method_def_param_names(
    app: &App,
    class_id: &ClassId,
    method: &Symbol,
) -> Option<Vec<Symbol>> {
    if let Some(c) = find_controller(app, class_id) {
        if let Some(a) = c.actions().find(|a| &a.name == method) {
            return Some(a.params.fields.keys().cloned().collect());
        }
    }
    if let Some(lc) = app.library_classes.iter().find(|lc| &lc.name == class_id) {
        if let Some(m) = lc.methods.iter().find(|m| &m.name == method) {
            return Some(m.params.iter().map(|p| p.name.clone()).collect());
        }
    }
    None
}

use serde_json::json;

/// Render a trace + its gap report as the structured JSON the #63
/// design sketches: `route` / `hops` / `coverage` / `gaps`. Optional
/// and empty fields are omitted, and hop order is the chain order.
/// `pub` because the wasm /ide/ skin serializes the identical shape —
/// one query layer, one wire format, N skins.
pub fn trace_json(trace: &Trace, report: &TraceGapReport) -> serde_json::Value {
    let hops: Vec<serde_json::Value> = trace.hops.iter().map(hop_json).collect();
    let gaps: Vec<serde_json::Value> = report
        .gaps
        .iter()
        .map(|g| {
            let mut o = json!({
                "kind": match g.kind {
                    TraceGapKind::UntypedBoundary => "untyped_boundary",
                    TraceGapKind::IngestGap => "ingest_gap",
                },
                "boundary": g.boundary,
                "blocked_hops": g.blocked_hops,
                "detail": g.detail,
            });
            if let Some(c) = &g.candidate_rbs {
                let m = o.as_object_mut().unwrap();
                m.insert("candidate_rbs".into(), json!(c));
                m.insert("accept".into(), json!("write the signature to sig/<file>.rbs — it is read back on the next analysis"));
            }
            o
        })
        .collect();
    json!({
        "route": trace.route,
        "controller": trace.controller,
        "action": trace.action,
        "hops": hops,
        "coverage": {
            "resolved_hops": report.resolved_hops,
            "total_hops": report.total_hops,
            "complete": report.complete(),
        },
        "gaps": gaps,
    })
}

fn hop_json(hop: &TraceHop) -> serde_json::Value {
    use TraceHop::*;
    fn set(o: &mut serde_json::Value, key: &str, v: serde_json::Value) {
        o.as_object_mut().unwrap().insert(key.into(), v);
    }
    fn set_loc(o: &mut serde_json::Value, file: &Option<String>, line: Option<u32>) {
        if let Some(f) = file {
            set(o, "file", json!(f));
            if let Some(l) = line {
                set(o, "line", json!(l));
            }
        }
    }
    fn assigns_json(assigns: &[(String, String)]) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (k, v) in assigns {
            m.insert(k.clone(), json!(v));
        }
        serde_json::Value::Object(m)
    }
    fn set_n_plus_one(o: &mut serde_json::Value, findings: &[PreloadFinding]) {
        if findings.is_empty() {
            return;
        }
        let arr: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                let mut e = json!({ "association": f.association, "message": f.message });
                let m = e.as_object_mut().unwrap();
                if let Some(file) = &f.file {
                    m.insert("file".into(), json!(file));
                    if let Some(l) = f.line {
                        m.insert("line".into(), json!(l));
                    }
                }
                if let Some(q) = &f.query_site {
                    m.insert("query_site".into(), json!(q));
                }
                e
            })
            .collect();
        set(o, "n_plus_one", json!(arr));
    }
    match hop {
        Route { method, path, params } => {
            let mut o = json!({ "kind": "route", "method": method, "path": path });
            if !params.is_empty() {
                set(&mut o, "binds", json!(params));
            }
            o
        }
        Filter(f) => {
            let mut o = json!({
                "kind": "filter",
                "filter_kind": f.filter_kind,
                "name": f.name,
                "defined_in": f.defined_in,
                "applies": f.applies,
                "resolved": f.resolved,
            });
            if f.included_via != f.defined_in {
                set(&mut o, "included_via", json!(f.included_via));
            }
            set_loc(&mut o, &f.file, f.line);
            if let Some(c) = &f.condition {
                set(&mut o, "condition", json!(c));
            }
            if let Some((verb, runs)) = &f.decided {
                set(&mut o, "decided", json!({ "verb": verb, "runs": runs }));
            }
            if !f.only.is_empty() {
                set(&mut o, "only", json!(f.only));
            }
            if !f.except.is_empty() {
                set(&mut o, "except", json!(f.except));
            }
            if let Some(sk) = &f.skipped_by {
                set(&mut o, "skipped_by", json!(sk));
            }
            if !f.assigns.is_empty() {
                set(&mut o, "assigns", assigns_json(&f.assigns));
            }
            if !f.effects.is_empty() {
                set(&mut o, "effects", json!(f.effects));
            }
            set_n_plus_one(&mut o, &f.n_plus_one);
            o
        }
        Action { name, controller, file, line, formats, assigns, effects, n_plus_one } => {
            let mut o = json!({ "kind": "action", "name": name, "controller": controller });
            set_loc(&mut o, file, *line);
            if !formats.is_empty() {
                set(&mut o, "formats", json!(formats));
            }
            if !assigns.is_empty() {
                set(&mut o, "assigns", assigns_json(assigns));
            }
            if !effects.is_empty() {
                set(&mut o, "effects", json!(effects));
            }
            set_n_plus_one(&mut o, n_plus_one);
            o
        }
        Response { detail } => json!({ "kind": "response", "detail": detail }),
        View { name, file, partials, n_plus_one } => {
            let mut o = json!({ "kind": "view", "name": name });
            set_loc(&mut o, file, None);
            if !partials.is_empty() {
                set(&mut o, "partials", json!(partials));
            }
            set_n_plus_one(&mut o, n_plus_one);
            o
        }
        Layout { name, file, n_plus_one } => {
            let mut o = json!({ "kind": "layout", "name": name });
            set_loc(&mut o, file, None);
            set_n_plus_one(&mut o, n_plus_one);
            o
        }
    }
}

// ── Trace text skins ─────────────────────────────────────────────────
//
// The LSP has no trace panel: a CodeLens above each action carries a
// one-line summary, and clicking it opens the chain as a document.
// Both are plain renderings of the same `Trace` + `TraceGapReport`
// the JSON skin serializes, so the editor and the /ide/ never
// disagree about a hop.

/// One line for a CodeLens title: `GET /rooms/:id · 7 filters · 2 skipped
/// · 1 N+1 · 1 unresolved`. Counts only what is non-zero; an action with
/// no filters and a complete chain reads just `GET /rooms/:id · trace
/// complete`.
pub fn trace_summary(trace: &Trace, report: &TraceGapReport) -> String {
    let route = match trace.hops.iter().find_map(|h| match h {
        TraceHop::Route { method, path, .. } => Some(format!("{method} {path}")),
        _ => None,
    }) {
        Some(r) => r,
        None => "unrouted".to_string(),
    };
    let filters = trace.hops.iter().filter(|h| matches!(h, TraceHop::Filter(_))).count();
    let skipped = trace
        .hops
        .iter()
        .filter(|h| matches!(h, TraceHop::Filter(f) if !f.applies))
        .count();
    let n_plus_one = trace_n_plus_one(trace).len();
    let mut parts = vec![route];
    if filters > 0 {
        parts.push(format!("{filters} filter{}", if filters == 1 { "" } else { "s" }));
    }
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
    }
    if n_plus_one > 0 {
        parts.push(format!("{n_plus_one} N+1"));
    }
    if report.complete() {
        parts.push("trace complete".to_string());
    } else {
        parts.push(format!("{}/{} hops resolved", report.resolved_hops, report.total_hops));
    }
    parts.join(" · ")
}

/// Every N+1 finding on the trace, in hop order (applying hops only —
/// a gated-out filter carries none).
pub fn trace_n_plus_one(trace: &Trace) -> Vec<&PreloadFinding> {
    trace
        .hops
        .iter()
        .flat_map(|h| match h {
            TraceHop::Filter(f) => f.n_plus_one.iter(),
            TraceHop::Action { n_plus_one, .. }
            | TraceHop::View { n_plus_one, .. }
            | TraceHop::Layout { n_plus_one, .. } => n_plus_one.iter(),
            TraceHop::Route { .. } | TraceHop::Response { .. } => [].iter(),
        })
        .collect()
}

/// The chain as a Markdown document — what the LSP opens when the
/// CodeLens is clicked. Hop order is chain order; gated-out filters
/// are struck through with their reason; the footer is the gap
/// report, phrased exactly as the /ide/ panel phrases it.
pub fn trace_text(trace: &Trace, report: &TraceGapReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", trace.route));
    let loc = |file: &Option<String>, line: Option<u32>| -> String {
        match (file, line) {
            (Some(f), Some(l)) => format!(" — {f}:{l}"),
            (Some(f), None) => format!(" — {f}"),
            _ => String::new(),
        }
    };
    let assigns = |a: &[(String, String)]| -> String {
        if a.is_empty() {
            String::new()
        } else {
            let rendered: Vec<String> = a.iter().map(|(k, v)| format!("{k} : {v}")).collect();
            format!("  \n  assigns {}", rendered.join(", "))
        }
    };
    let findings = |f: &[PreloadFinding]| -> String {
        f.iter().map(|f| format!("  \n  **N+1** {}", f.message)).collect::<String>()
    };
    for hop in &trace.hops {
        match hop {
            TraceHop::Route { method, path, params } => {
                out.push_str(&format!("- **route** {method} {path}"));
                if !params.is_empty() {
                    out.push_str(&format!(" (binds {})", params.join(", ")));
                }
                out.push('\n');
            }
            TraceHop::Filter(f) => {
                let name = if f.applies { f.name.clone() } else { format!("~~{}~~", f.name) };
                out.push_str(&format!("- **{}** {name}", f.filter_kind));
                if f.included_via != f.defined_in {
                    out.push_str(&format!(" ({} via {})", f.defined_in, f.included_via));
                } else {
                    out.push_str(&format!(" ({})", f.defined_in));
                }
                if let Some(c) = &f.condition {
                    out.push_str(&format!(" `{c}`"));
                }
                if let Some((verb, runs)) = &f.decided {
                    out.push_str(&format!(" — {} for {verb}", if *runs { "runs" } else { "skipped" }));
                } else if let Some(sk) = &f.skipped_by {
                    out.push_str(&format!(" — skipped by {sk}"));
                } else if !f.applies {
                    if !f.only.is_empty() {
                        out.push_str(&format!(" — only: {}", f.only.join(", ")));
                    } else if !f.except.is_empty() {
                        out.push_str(&format!(" — except: {}", f.except.join(", ")));
                    }
                }
                if f.applies && !f.resolved {
                    out.push_str(" — unresolved");
                }
                out.push_str(&loc(&f.file, f.line));
                out.push_str(&assigns(&f.assigns));
                out.push_str(&findings(&f.n_plus_one));
                out.push('\n');
            }
            TraceHop::Action { name, controller, file, line, formats, assigns: a, n_plus_one, .. } => {
                out.push_str(&format!("- **action** {controller}#{name}"));
                if !formats.is_empty() {
                    out.push_str(&format!(" ({})", formats.join(" · ")));
                }
                out.push_str(&loc(file, *line));
                out.push_str(&assigns(a));
                out.push_str(&findings(n_plus_one));
                out.push('\n');
            }
            TraceHop::Response { detail } => out.push_str(&format!("- **response** {detail}\n")),
            TraceHop::View { name, file, partials, n_plus_one } => {
                out.push_str(&format!("- **view** {name}{}", loc(file, None)));
                for p in partials {
                    out.push_str(&format!("  \n  partial {p}"));
                }
                out.push_str(&findings(n_plus_one));
                out.push('\n');
            }
            TraceHop::Layout { name, file, n_plus_one } => {
                out.push_str(&format!("- **layout** {name}{}", loc(file, None)));
                out.push_str(&findings(n_plus_one));
                out.push('\n');
            }
        }
    }
    out.push('\n');
    if report.complete() {
        out.push_str(&format!("✓ trace complete — all {} hops resolved\n", report.total_hops));
    } else {
        out.push_str(&format!(
            "── {} gap(s) · {}/{} hops resolved ──\n",
            report.gaps.len(),
            report.resolved_hops,
            report.total_hops
        ));
        for g in &report.gaps {
            let tag = match g.kind {
                TraceGapKind::UntypedBoundary => "boundary",
                TraceGapKind::IngestGap => "tool",
            };
            out.push_str(&format!("- [{tag}] {} — {}\n", g.boundary, g.detail));
            if let Some(rbs) = &g.candidate_rbs {
                out.push_str(&format!("  \n  candidate RBS: `{rbs}` — write it to sig/<file>.rbs\n"));
            }
        }
    }
    out
}

/// The 0-based line of `def <action>` in `text`. The IR carries no span
/// for a method header, so the header is found in the source: the
/// nearest `def` for the name at or above `body_line` (0-based, the
/// body's first statement) when one is known, else the first such
/// `def` in the file. `None` when the file defines no such method.
pub fn action_def_line(text: &str, action: &str, body_line: Option<u32>) -> Option<u32> {
    let is_def = |line: &str| {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("def ") else { return false };
        let rest = rest.trim_start();
        // `def self.name` is a class method; not an action.
        rest.strip_prefix(action)
            .is_some_and(|tail| !tail.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '?' || c == '!' || c == '='))
    };
    let lines: Vec<&str> = text.lines().collect();
    match body_line {
        Some(b) => (0..=(b as usize).min(lines.len().saturating_sub(1)))
            .rev()
            .find(|&i| is_def(lines[i]))
            .map(|i| i as u32),
        None => lines.iter().position(|l| is_def(l)).map(|i| i as u32),
    }
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
    fn members_of_enumerates_columns_associations_and_finders() {
        let app = real_blog();
        let mut analyzer = Analyzer::new(&app);
        // Registry refinement (user-method returns) needs the fixpoint;
        // re-run analyze on a scratch copy so `app` stays borrowable.
        let mut typed = app.clone();
        analyzer.analyze(&mut typed);
        let registry = analyzer.class_registry();
        let article = ClassId(Symbol::from("Article"));

        let instance = members_of(&typed, registry, &article, MemberSide::Instance);
        let find = |name: &str| instance.iter().find(|m| m.name.as_str() == name);
        let title = find("title").expect("schema column `title`");
        assert_eq!(title.kind, MemberKind::Column);
        // `articles.title` has no `null: false`, so a read can be nil —
        // the attributes row carries the slot type and the hover says so.
        assert_eq!(title.display, "String?");
        assert!(find("title=").is_some(), "writer twin for the column");
        let comments = find("comments").expect("has_many :comments");
        assert_eq!(comments.kind, MemberKind::Association);
        assert_eq!(comments.display, "Array[Comment]");
        // Catalog-sourced AR instance surface rides along.
        assert!(find("save").is_some(), "AR catalog instance method");

        let class_side = members_of(&typed, registry, &article, MemberSide::Class);
        let findc = |name: &str| class_side.iter().find(|m| m.name.as_str() == name);
        let find_by = findc("find_by").expect("AR finder");
        assert_eq!(find_by.display, "Article?", "find_by is Self-or-nil");
        let create = findc("create").expect("AR create");
        assert_eq!(create.display, "Article");
        // Class side must not leak instance members and vice versa.
        assert!(findc("title").is_none(), "columns are instance-side");
        assert!(find("find_by").is_none(), "finders are class-side");
    }

    #[test]
    fn complete_at_offers_members_kwargs_and_ivars() {
        let app = real_blog();
        let mut analyzer = Analyzer::new(&app);
        let mut typed = app.clone();
        analyzer.analyze(&mut typed);
        let registry = analyzer.class_registry();
        let path = "fixtures/real-blog/app/controllers/articles_controller.rb";
        let text = source(&typed, file_id(&typed, path).unwrap()).unwrap().text.clone();

        // Constant receiver typed on a fresh line → class side.
        let edited = format!("{text}\nArticle.");
        let cands =
            complete_at(&typed, registry, path, &edited, edited.len()).expect("members");
        let find_by = cands.iter().find(|c| c.label == "find_by").expect("find_by");
        assert_eq!(find_by.detail, "Article?");
        assert!(cands.iter().all(|c| c.label != "title"), "columns are instance-side");

        // Kwargs inside find_by(.
        let edited = format!("{text}\nArticle.find_by(");
        let cands = complete_at(&typed, registry, path, &edited, edited.len()).expect("kwargs");
        let title = cands.iter().find(|c| c.label == "title:").expect("column kwarg");
        assert_eq!(title.kind, CandidateKind::Kwarg);
        assert_eq!(title.insert_text.as_deref(), Some("title: "));

        // Ivar receiver via the file-ivar fallback → instance side.
        let edited = format!("{text}\n@article.");
        let cands = complete_at(&typed, registry, path, &edited, edited.len()).expect("ivars");
        assert!(cands.iter().any(|c| c.label == "comments"));

        // `@` prefix lists the file's ivars.
        let edited = format!("{text}\n@art");
        let cands = complete_at(&typed, registry, path, &edited, edited.len()).expect("@");
        assert!(cands.iter().any(|c| c.label == "@article" && c.kind == CandidateKind::Ivar));
    }

    #[test]
    fn related_files_walks_the_render_graph() {
        let app = real_blog();
        let mut typed = app.clone();
        Analyzer::new(&app).analyze(&mut typed);

        let rel = related_files(&typed, "fixtures/real-blog/app/controllers/articles_controller.rb");
        assert!(
            rel.iter().any(|r| r.kind == RelatedKind::View && r.label == "articles/show"),
            "controller relates to the views it feeds; got {:?}",
            rel.iter().map(|r| (&r.label, r.kind)).collect::<Vec<_>>()
        );
        assert!(
            rel.iter().any(|r| r.kind == RelatedKind::Model && r.label == "Article"),
            "controller relates to its conventional model"
        );

        // A view relates back to its feeding controller and its partials
        // (`articles/new` statically renders the `_form` partial).
        let new_view = rel.iter().find(|r| r.label == "articles/new").unwrap();
        let rel = related_files(&typed, &new_view.path);
        assert!(rel
            .iter()
            .any(|r| r.kind == RelatedKind::Controller && r.label == "ArticlesController"));
        assert!(
            rel.iter()
                .any(|r| r.kind == RelatedKind::Partial && r.label == "articles/_form"),
            "articles/new renders _form; got {:?}",
            rel.iter().map(|r| (&r.label, r.kind)).collect::<Vec<_>>()
        );

        // And the partial knows its renderers.
        let form = rel.iter().find(|r| r.label == "articles/_form").unwrap();
        let rel = related_files(&typed, &form.path);
        assert!(
            rel.iter().any(|r| r.kind == RelatedKind::Renderer && r.label == "articles/new"),
            "partial relates to its renderers; got {:?}",
            rel.iter().map(|r| (&r.label, r.kind)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn nil_verdict_is_three_valued() {
        // Provable yes.
        assert_eq!(nil_verdict(Some(&Ty::Nil)), Some(true));
        assert_eq!(
            nil_verdict(Some(&Ty::Union { variants: vec![class("Article"), Ty::Nil] })),
            Some(true)
        );
        // Provable no.
        assert_eq!(nil_verdict(Some(&class("Article"))), Some(false));
        assert_eq!(nil_verdict(Some(&Ty::Int)), Some(false));
        // Can't tell: untyped, unresolved var, unions carrying either,
        // and positions the analyzer never typed at all.
        assert_eq!(nil_verdict(Some(&Ty::Untyped)), None);
        assert_eq!(nil_verdict(Some(&Ty::Var { var: TyVar(0) })), None);
        assert_eq!(
            nil_verdict(Some(&Ty::Union { variants: vec![Ty::Untyped, Ty::Str] })),
            None
        );
        assert_eq!(nil_verdict(None), None);
        // An unknown arm doesn't retract a proven nil arm.
        assert_eq!(
            nil_verdict(Some(&Ty::Union { variants: vec![Ty::Untyped, Ty::Nil] })),
            Some(true)
        );
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

    /// An offset pointing INTO a character clamps to that character's
    /// start instead of panicking on a non-boundary slice. Every
    /// interior byte of a template's em dash is such an offset.
    #[test]
    fn position_clamps_inside_a_multibyte_char() {
        let text = "a — b";
        // "—" occupies bytes 2..5.
        let start = offset_to_position(text, 2);
        assert_eq!(start, Position { line: 0, character: 2 });
        assert_eq!(offset_to_position(text, 3), start);
        assert_eq!(offset_to_position(text, 4), start);
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
        // Every write is in the controller — and the reads span the
        // templates its actions feed (Rails' controller→view channel).
        assert!(refs.iter().filter(|r| r.write).all(|r| r.span.file == file), "writes are the controller's");
        let in_views = refs
            .iter()
            .filter(|r| source(&app, r.span.file).is_some_and(|s| s.path.contains("app/views/")))
            .count();
        assert!(in_views > 0, "reads in the fed templates are references too");
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

    #[test]
    fn traceroute_composes_route_filters_action_view_layout() {
        let app = real_blog();
        let trace = traceroute(&app, "ArticlesController#edit").expect("trace");
        assert_eq!(trace.route, "GET /articles/:id/edit → ArticlesController#edit");

        // Route hop binds :id.
        assert!(matches!(
            &trace.hops[0],
            TraceHop::Route { method, params, .. }
                if method == "GET" && params == &vec!["id".to_string()]
        ));

        // set_article applies to show (only: gating), assigns @article.
        let set_article = trace
            .hops
            .iter()
            .find_map(|h| match h {
                TraceHop::Filter(f) if f.name == "set_article" => Some(f),
                _ => None,
            })
            .expect("set_article hop");
        assert!(set_article.applies);
        assert_eq!(set_article.defined_in, "ArticlesController");
        assert_eq!(set_article.included_via, "ArticlesController");
        assert!(set_article.file.as_deref().is_some_and(|f| f.ends_with("articles_controller.rb")));
        assert!(
            set_article.assigns.iter().any(|(k, v)| k == "@article" && v == "Article"),
            "assigns = {:?}",
            set_article.assigns
        );
        assert!(
            set_article.effects.iter().any(|e| e.starts_with("DbRead")),
            "Article.find → DbRead; effects = {:?}",
            set_article.effects
        );

        // Action, view (with the _article partial via render @article),
        // and the convention layout, in that order after the filters.
        let kinds: Vec<&str> = trace
            .hops
            .iter()
            .map(|h| match h {
                TraceHop::Route { .. } => "route",
                TraceHop::Filter(_) => "filter",
                TraceHop::Action { .. } => "action",
                TraceHop::Response { .. } => "response",
                TraceHop::View { .. } => "view",
                TraceHop::Layout { .. } => "layout",
            })
            .collect();
        assert_eq!(kinds.first(), Some(&"route"));
        assert!(kinds.ends_with(&["action", "view", "layout"]), "kinds = {kinds:?}");

        let view = trace
            .hops
            .iter()
            .find_map(|h| match h {
                TraceHop::View { name, partials, .. } => Some((name, partials)),
                _ => None,
            })
            .expect("view hop");
        assert_eq!(view.0, "articles/edit");
        // edit renders the shared `_form` partial (string-form render —
        // record/collection renders don't produce render_edges yet).
        assert!(
            view.1.iter().any(|p| p == "articles/_form"),
            "partials = {:?}",
            view.1
        );
        assert!(trace.hops.iter().any(|h| matches!(
            h,
            TraceHop::Layout { name, .. } if name == "layouts/application"
        )));
    }


    fn tree_app(files: &[(&str, &str)]) -> App {
        let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = files
            .iter()
            .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
            .collect();
        let (ir, _) =
            crate::ingest::prism::scope(|| crate::ingest::ingest_app_from_tree(tree));
        let mut app = ir.expect("tree ingest");
        Analyzer::new(&app).analyze(&mut app);
        app
    }

    /// The filter chain runs in REGISTRATION order, which for
    /// `include A, B, C` is C, B, A (Ruby's `Module#include` takes its
    /// arguments last-first), with a concern's own concern dependencies
    /// ahead of it (ActiveSupport::Concern includes them before running
    /// the `included` block). And a block-form filter in a concern's
    /// `included do` is a hop like any other — placed, attributed to the
    /// concern, resolved to its own line, named by its code. campfire's
    /// ApplicationController is the shape: `include AllowBrowser,
    /// Authentication, …, SetCurrentRequest, …, VersionHeaders` ran
    /// `allow_browser` first in the trace and never listed
    /// `Current.request = request` at all.
    #[test]
    fn traceroute_orders_concern_filters_by_registration_and_keeps_block_filters() {
        let app = tree_app(&[
            (
                "app/controllers/concerns/alpha.rb",
                "module Alpha
  extend ActiveSupport::Concern
  include AlphaDep
  included do
    before_action :from_alpha
  end
  private
    def from_alpha
      @alpha = 1
    end
end
",
            ),
            (
                "app/controllers/concerns/alpha_dep.rb",
                "module AlphaDep
  extend ActiveSupport::Concern
  included do
    before_action :from_alpha_dep
  end
  private
    def from_alpha_dep
      @dep = 1
    end
end
",
            ),
            (
                "app/controllers/concerns/set_current_request.rb",
                "module SetCurrentRequest
  extend ActiveSupport::Concern
  included do
    before_action do
      Current.request = request
    end
  end
end
",
            ),
            (
                "app/controllers/concerns/omega.rb",
                "module Omega
  extend ActiveSupport::Concern
  included do
    before_action :from_omega
  end
  private
    def from_omega
      @omega = 1
    end
end
",
            ),
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base
  include Alpha, SetCurrentRequest, Omega
end
",
            ),
            (
                "app/controllers/widgets_controller.rb",
                "class WidgetsController < ApplicationController
  before_action :own
  def index
  end
  private
    def own
      @own = 1
    end
end
",
            ),
            ("app/views/widgets/index.html.erb", "<p>hi</p>\n"),
            ("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: :index\nend\n"),
        ]);
        let trace = traceroute(&app, "WidgetsController#index").expect("trace");
        let filters: Vec<&FilterHop> = trace
            .hops
            .iter()
            .filter_map(|h| match h {
                TraceHop::Filter(f) => Some(f),
                _ => None,
            })
            .collect();
        let names: Vec<&str> = filters.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "from_omega",
                "before_action { Current.request = request }",
                "from_alpha_dep",
                "from_alpha",
                "own",
            ],
            "registration order: last include argument first, dependency before dependent, own last"
        );
        let block = filters[1];
        assert_eq!(block.defined_in, "SetCurrentRequest");
        assert_eq!(block.included_via, "ApplicationController");
        assert!(block.resolved, "a block-form hop resolves to its own code");
        assert!(block.applies);
        assert!(block.file.as_deref().is_some_and(|f| f.ends_with("set_current_request.rb")), "{:?}", block.file);
        assert_eq!(block.line, Some(5), "the block body, not the `before_action do` line");
        let report = trace_gap_report(&app, &trace, &[], None);
        assert!(report.complete(), "gaps = {:?}", report.gaps);
    }

    /// A guard that reads only the request verb is decided by the
    /// route: campfire's `reject_banned_ip unless: :safe_request?` with
    /// `safe_request? = request.get? || request.head?` does not run on
    /// GET and does on POST. A guard reading anything else stays
    /// undecided (hop applies, condition carried as text).
    #[test]
    fn traceroute_decides_verb_only_guards_from_the_route() {
        let app = tree_app(&[
            (
                "app/controllers/concerns/block_banned_requests.rb",
                "module BlockBannedRequests
  extend ActiveSupport::Concern
  included do
    before_action :reject_banned_ip, unless: :safe_request?
    before_action :audit, if: :audited?
  end
  private
    def safe_request?
      request.get? || request.head?
    end
    def audited?
      params[:audit].present?
    end
    def reject_banned_ip
      head :forbidden
    end
    def audit
    end
end
",
            ),
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base
  include BlockBannedRequests
end
",
            ),
            (
                "app/controllers/widgets_controller.rb",
                "class WidgetsController < ApplicationController
  def index
  end
  def create
    head :created
  end
end
",
            ),
            ("app/views/widgets/index.html.erb", "<p>hi</p>\n"),
            ("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :create]\nend\n"),
        ]);
        let hop = |query: &str, name: &str| -> FilterHop {
            let trace = traceroute(&app, query).expect("trace");
            trace
                .hops
                .iter()
                .find_map(|h| match h {
                    TraceHop::Filter(f) if f.name == name => Some(f.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{name} hop in {query}"))
        };
        let on_get = hop("GET /widgets", "reject_banned_ip");
        assert_eq!(on_get.decided, Some(("GET".to_string(), false)));
        assert!(!on_get.applies, "a GET is a safe request: the filter does not run");
        assert_eq!(on_get.condition.as_deref(), Some("unless: :safe_request?"));
        assert!(on_get.skipped_by.is_none(), "decided by the guard, not a skip");

        let on_post = hop("POST /widgets", "reject_banned_ip");
        assert_eq!(on_post.decided, Some(("POST".to_string(), true)));
        assert!(on_post.applies);

        // Not a verb predicate: no verdict, hop applies as before.
        let audit = hop("GET /widgets", "audit");
        assert_eq!(audit.decided, None);
        assert!(audit.applies);
        assert_eq!(audit.condition.as_deref(), Some("if: :audited?"));
    }

    /// Two templates with one view name — `_message.html.erb` and
    /// `_message.json.jbuilder` are both `messages/_message` — union
    /// their render edges. campfire's jbuilder (no renders) was walked
    /// after the ERB and erased its three partials from the graph.
    #[test]
    fn same_named_templates_union_their_render_edges() {
        let app = tree_app(&[
            (
                "app/controllers/messages_controller.rb",
                "class MessagesController < ApplicationController
  def index
    @messages = Message.all
  end
end
",
            ),
            ("app/models/message.rb", "class Message < ApplicationRecord
end
"),
            ("db/schema.rb", "ActiveRecord::Schema[8.0].define(version: 1) do
  create_table \"messages\" do |t|
    t.string \"body\"
  end
end
"),
            ("config/routes.rb", "Rails.application.routes.draw do
  resources :messages, only: :index
end
"),
            ("app/views/messages/index.html.erb", "<%= render partial: \"messages/message\", collection: @messages %>\n"),
            ("app/views/messages/_message.html.erb", "<%= render \"messages/actions\", message: message %>\n"),
            ("app/views/messages/_message.json.jbuilder", "json.id message.id\n"),
            ("app/views/messages/_actions.html.erb", "<p><%= message.body %></p>\n"),
        ]);
        let edges = app.render_edges.get(&Symbol::from("messages/_message")).cloned().unwrap_or_default();
        assert!(
            edges.contains(&Symbol::from("messages/_actions")),
            "the ERB's edge survives the jbuilder twin: {edges:?}"
        );
        let rel = related_files(&app, "app/views/messages/_actions.html.erb");
        assert!(
            rel.iter().any(|r| r.kind == RelatedKind::Renderer && r.label == "messages/_message"),
            "the partial knows its renderer; got {:?}",
            rel.iter().map(|r| (&r.label, r.kind)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn gap_report_complete_when_every_hop_resolves() {
        let app = real_blog();
        let trace = traceroute(&app, "ArticlesController#show").expect("trace");
        let report = trace_gap_report(&app, &trace, &[], None);
        assert!(report.complete(), "gaps = {:?}", report.gaps);
        assert!(report.total_hops >= 2, "filter + action at minimum");
        assert_eq!(report.resolved_hops, report.total_hops);
    }

    #[test]
    fn gap_report_prices_unresolved_gem_boundary() {
        let app = tree_app(&[
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\n  before_action :authenticate_user!\nend\n",
            ),
            (
                "app/controllers/widgets_controller.rb",
                "class WidgetsController < ApplicationController\n  def index\n  end\nend\n",
            ),
            ("app/views/widgets/index.html.erb", "<p>hi</p>\n"),
            (
                "db/schema.rb",
                "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n",
            ),
        ]);
        let trace = traceroute(&app, "WidgetsController#index").expect("trace");
        // The hop itself is marked unresolved…
        let hop = trace
            .hops
            .iter()
            .find_map(|h| match h {
                TraceHop::Filter(f) if f.name == "authenticate_user!" => Some(f),
                _ => None,
            })
            .expect("hop present");
        assert!(hop.applies && !hop.resolved);

        // …and the footer prices it as a user-actionable boundary
        // (no ingest gap recorded for that file → not ours).
        let report = trace_gap_report(&app, &trace, &[], None);
        assert!(!report.complete());
        let gap = report
            .gaps
            .iter()
            .find(|g| g.boundary == "ApplicationController#authenticate_user!")
            .expect("boundary entry");
        assert_eq!(gap.kind, TraceGapKind::UntypedBoundary);
        assert_eq!(gap.blocked_hops, 1);
        assert!(gap.candidate_rbs.is_none(), "nothing inferred for a gem method");
        assert_eq!(report.resolved_hops, report.total_hops - 1);
    }

    #[test]
    fn gap_report_attributes_ingest_gap_files_as_tool_coverage() {
        let app = real_blog();
        let trace = traceroute(&app, "ArticlesController#show").expect("trace");
        // Synthesize a recorded gap in a file the trace runs through.
        let gap = crate::ingest::IngestError::Unsupported {
            file: "fixtures/real-blog/app/controllers/articles_controller.rb".to_string(),
            message: "unsupported expression node: FooNode".to_string(),
        };
        let report = trace_gap_report(&app, &trace, &[gap], None);
        let entry = report
            .gaps
            .iter()
            .find(|g| g.kind == TraceGapKind::IngestGap)
            .expect("tool-coverage entry");
        assert!(entry.boundary.ends_with("articles_controller.rb"));
        assert!(entry.detail.contains("tool coverage, not your code"));
    }

    #[test]
    fn candidate_signature_synthesizes_from_inferred_types() {
        let app = tree_app(&[
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\nend\n",
            ),
            (
                "app/models/price_calculator.rb",
                "class PriceCalculator\n  def total(count)\n    count * 3\n  end\nend\n",
            ),
            (
                "app/controllers/widgets_controller.rb",
                "class WidgetsController < ApplicationController\n  def index\n    @price = PriceCalculator.new.total(4)\n  end\nend\n",
            ),
            ("app/views/widgets/index.html.erb", "<p><%= @price %></p>\n"),
            (
                "db/schema.rb",
                "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n",
            ),
        ]);
        let analyzer = {
            let mut a = Analyzer::new(&app);
            let mut scratch = app.clone();
            a.analyze(&mut scratch);
            a
        };
        let sig = candidate_signature(
            &app,
            &analyzer,
            &ClassId(Symbol::from("PriceCalculator")),
            &Symbol::from("total"),
        );
        assert_eq!(sig.as_deref(), Some("def total: (Integer count) -> Integer"));
    }

    #[test]
    fn traceroute_annotates_hops_with_missing_preload_findings() {
        // #63 phase 5: one un-preloaded chain iterated in a filter, an
        // action, and the template — the finding lands on each hop
        // whose body/template contains the access site.
        let app = tree_app(&[
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\nend\n",
            ),
            (
                "app/controllers/articles_controller.rb",
                r#"class ArticlesController < ApplicationController
  before_action :audit_articles

  def index
    @articles = Article.order(:title)
    @articles.each { |a| a.comments }
  end

  def audit_articles
    Article.order(:title).each { |a| a.comments }
  end
end
"#,
            ),
            (
                "app/models/article.rb",
                "class Article < ApplicationRecord\n  has_many :comments\nend\n",
            ),
            (
                "app/models/comment.rb",
                "class Comment < ApplicationRecord\n  belongs_to :article\nend\n",
            ),
            (
                "app/views/articles/index.html.erb",
                "<% @articles.each do |a| %><%= a.comments.size %><% end %>\n",
            ),
            (
                "db/schema.rb",
                r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "title"
  end
  create_table "comments", force: :cascade do |t|
    t.integer "article_id"
    t.text "body"
  end
end
"#,
            ),
        ]);
        let trace = traceroute(&app, "ArticlesController#index").expect("trace");

        let filter = trace
            .hops
            .iter()
            .find_map(|h| match h {
                TraceHop::Filter(f) if f.name == "audit_articles" => Some(f),
                _ => None,
            })
            .expect("audit_articles hop");
        assert_eq!(filter.n_plus_one.len(), 1, "filter-body finding: {:?}", filter.n_plus_one);
        assert_eq!(filter.n_plus_one[0].association, "comments");

        let action = trace
            .hops
            .iter()
            .find_map(|h| match h {
                TraceHop::Action { n_plus_one, .. } => Some(n_plus_one),
                _ => None,
            })
            .expect("action hop");
        assert_eq!(action.len(), 1, "action-body finding: {action:?}");
        assert!(action[0].file.as_deref().is_some_and(|f| f.ends_with("articles_controller.rb")));

        let view = trace
            .hops
            .iter()
            .find_map(|h| match h {
                TraceHop::View { n_plus_one, .. } => Some(n_plus_one),
                _ => None,
            })
            .expect("view hop");
        assert_eq!(view.len(), 1, "template finding via the ivar channel: {view:?}");
        assert!(view[0].file.as_deref().is_some_and(|f| f.ends_with("index.html.erb")));
        assert!(
            view[0].query_site.as_deref().is_some_and(|q| q.contains("articles_controller.rb")),
            "query site names the controller; got {:?}",
            view[0].query_site
        );

        // The wire shape carries the annotation; clean hops omit it.
        let report = trace_gap_report(&app, &trace, &[], None);
        let v = trace_json(&trace, &report);
        let hops = v["hops"].as_array().expect("hops");
        let jf = hops
            .iter()
            .find(|h| h["kind"] == "filter" && h["name"] == "audit_articles")
            .expect("filter hop json");
        assert_eq!(jf["n_plus_one"][0]["association"], "comments");
        assert!(jf["n_plus_one"][0]["query_site"].as_str().is_some());
        let jv = hops.iter().find(|h| h["kind"] == "view").expect("view hop json");
        assert!(jv["n_plus_one"].is_array());
    }

    #[test]
    fn traceroute_hops_stay_clean_when_the_chain_preloads() {
        let app = real_blog();
        let trace = traceroute(&app, "ArticlesController#index").expect("trace");
        for hop in &trace.hops {
            let (kind, n) = match hop {
                TraceHop::Filter(f) => ("filter", &f.n_plus_one),
                TraceHop::Action { n_plus_one, .. } => ("action", n_plus_one),
                TraceHop::View { n_plus_one, .. } => ("view", n_plus_one),
                TraceHop::Layout { n_plus_one, .. } => ("layout", n_plus_one),
                _ => continue,
            };
            assert!(n.is_empty(), "unexpected N+1 on {kind} hop: {n:?}");
        }
    }

    #[test]
    fn traceroute_marks_gated_out_filters_and_resolves_routes() {
        let app = real_blog();
        // index is not in set_article's only: list → hop kept, applies=false.
        let trace = traceroute(&app, "ArticlesController#index").expect("trace");
        let set_article = trace
            .hops
            .iter()
            .find_map(|h| match h {
                TraceHop::Filter(f) if f.name == "set_article" => Some(f),
                _ => None,
            })
            .expect("gated-out hop still present");
        assert!(!set_article.applies);
        assert!(set_article.skipped_by.is_none(), "gating is not a skip");

        // Route-form query resolves to the same chain.
        let by_route = traceroute(&app, "GET /articles/:id").expect("route query");
        assert_eq!(by_route.controller, "ArticlesController");
        assert_eq!(by_route.action, "show");

        // Unknown query → None.
        assert!(traceroute(&app, "NopeController#zap").is_none());
    }
}

