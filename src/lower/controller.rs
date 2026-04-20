//! Controller-body lowering — shared Phase 4c analysis.
//!
//! The four Phase-4c emitters (Rust, Crystal, Go, Elixir) each wanted
//! to match Ruby controller-body `Send` shapes and rewrite them into
//! a target-specific runtime call. The IR-match logic was identical;
//! only the rendering varied.
//!
//! This module exposes both halves of the shared piece:
//!
//! - **Predicates + walkers** (`split_public_private`,
//!   `walk_controller_ivars`, `is_query_builder_method`,
//!   `singularize_to_model`, `chain_target_class`, `is_params_expr`,
//!   `is_format_binding`) — building blocks each emitter can pull
//!   into its rendering pipeline.
//!
//! - **`SendKind` classifier** (`classify_controller_send`) — the
//!   lowered view of every Send shape each emitter cares about.
//!   Takes the raw `recv / method / args / block` and returns a
//!   tagged variant; the emitter's render table then produces
//!   target syntax. Unclassified Sends return `None` and fall
//!   through to the emitter's normal path (plain `recv.method(args)`
//!   rendering, self-dispatch, etc.).
//!
//! Variants live here when the shape appears in at least three of
//! the four emitters — validation that they're shape-shaped, not
//! target-shaped. Target-specific rewrites (Elixir's struct-method-
//! to-Module-function conversion) stay in the emitter.

use std::collections::BTreeSet;

use crate::App;
use crate::dialect::{Action, Controller, ControllerBodyItem, Filter, FilterKind, RouteSpec};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::naming;

/// Walk a controller's source-ordered body, partitioning actions into
/// those before the `private` marker vs. those after. Filters and
/// Unknown class-body calls are informational-only for emit and get
/// dropped; PrivateMarker is consumed as the partition point.
pub fn split_public_private(c: &Controller) -> (Vec<Action>, Vec<Action>) {
    let mut pubs = Vec::new();
    let mut privs = Vec::new();
    let mut seen_private = false;
    for item in &c.body {
        match item {
            ControllerBodyItem::PrivateMarker { .. } => seen_private = true,
            ControllerBodyItem::Action { action, .. } => {
                if seen_private {
                    privs.push(action.clone());
                } else {
                    pubs.push(action.clone());
                }
            }
            _ => {}
        }
    }
    (pubs, privs)
}

/// Walk an action body collecting every ivar it touches. Returns two
/// sets (both deterministic):
///
/// - `assigned`: ivar names that appear on the LHS of an assignment
///   at some point in the body.
/// - `referenced`: ivar names in first-use order — every read *or*
///   write registers here. Used by the Rust emitter to compute
///   "referenced but never assigned" (the Rails `before_action`
///   filter would set these in the real runtime; Phase 4c primes them
///   with defaults).
///
/// Callers that only need the referenced list (Crystal, Go) pull
/// that half and ignore `assigned`.
pub fn walk_controller_ivars(body: &Expr) -> WalkedIvars {
    let mut out = WalkedIvars::default();
    walk(body, &mut out);
    out
}

#[derive(Default, Debug, Clone)]
pub struct WalkedIvars {
    /// ivar names that appear as the LHS of an assignment.
    pub assigned: BTreeSet<Symbol>,
    /// ivar names in first-use order across the body (read or write).
    pub referenced: Vec<Symbol>,
    /// Fast-lookup mirror of `referenced` to keep insertions O(log n)
    /// without losing ordering.
    seen: BTreeSet<Symbol>,
}

impl WalkedIvars {
    pub fn ivars_read_without_assign(&self) -> Vec<Symbol> {
        self.referenced
            .iter()
            .filter(|n| !self.assigned.contains(*n))
            .cloned()
            .collect()
    }
}

fn walk(e: &Expr, out: &mut WalkedIvars) {
    match &*e.node {
        ExprNode::Ivar { name } => {
            if out.seen.insert(name.clone()) {
                out.referenced.push(name.clone());
            }
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            out.assigned.insert(name.clone());
            if out.seen.insert(name.clone()) {
                out.referenced.push(name.clone());
            }
            walk(value, out);
        }
        ExprNode::Assign { value, .. } => walk(value, out),
        ExprNode::Seq { exprs } => {
            for child in exprs {
                walk(child, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, out);
            walk(then_branch, out);
            walk(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, out);
            walk(right, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, out);
            }
            for a in args {
                walk(a, out);
            }
            if let Some(b) = block {
                walk(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, out);
                walk(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk(el, out);
            }
        }
        ExprNode::Lambda { body, .. } => walk(body, out),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    walk(expr, out);
                }
            }
        }
        _ => {}
    }
}

// `is_query_builder_method` moved to `crate::catalog`. It's a
// runtime-capability concern (which AR methods the scaffold
// runtime stubs implement as collapse-to-empty) that will
// eventually become a `DatabaseAdapter` trait method. For now,
// re-export so existing callers compile unchanged.
pub use crate::catalog::is_query_builder_method;

/// Resolve a HasMany association name to its target model class.
/// `"comments"` → `"Comment"` iff `Comment` is in `known_models`.
///
/// Used by the `.build(hash)` / `.create(hash)` / `<assoc>.find(x)`
/// rewrites in every Phase-4c emitter — they all need to default-
/// construct the target, and the target's name falls out of
/// singularising the method name on the association chain.
pub fn singularize_to_model(assoc: &str, known_models: &[Symbol]) -> Option<Symbol> {
    let class = naming::singularize_camelize(assoc);
    known_models
        .iter()
        .find(|m| m.as_str() == class)
        .cloned()
}

/// Walk a chain of `Send`s left until hitting a `Const { path }`.
/// Returns the final path segment (the presumed class name) when it's
/// a known model. Used to pick the element type for
/// `Vec::<T>::new()` / `[] of T` / `[]*T{}` chain punts.
pub fn chain_target_class(e: &Expr, known_models: &[Symbol]) -> Option<Symbol> {
    let mut cur = e;
    loop {
        match &*cur.node {
            ExprNode::Const { path } => {
                let class = path.last()?;
                return known_models
                    .iter()
                    .find(|m| m.as_str() == class.as_str())
                    .cloned();
            }
            ExprNode::Send { recv: Some(r), .. } => cur = r,
            _ => return None,
        }
    }
}

/// True when an expression references the implicit `params` object —
/// a bare `Send { recv: None, method: "params", args: [] }`. Used by
/// the `params.expect(...)` / `params[k]` rewrites in every
/// Phase-4c emitter.
pub fn is_params_expr(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "params" && args.is_empty()
    )
}

/// True when an expression is the block parameter bound by
/// `respond_to do |format|` — today just the local `format` var. Used
/// to disambiguate `format.html { ... }` inside a respond_to block
/// from any unrelated `x.html` call outside.
pub fn is_format_binding(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Var { name, .. } if name.as_str() == "format"
    )
}

// -- SendKind classifier --------------------------------------------
//
// Each variant names a controller-body Send shape that at least three
// of the four Phase-4c emitters (Rust / Crystal / Go / Elixir) handle.
// The classifier extracts the *intent* from the IR; the per-target
// emitter's render table produces the target syntax. Variants carry
// references into the original `Send` so the emitter can keep using
// its own ctx to render args/recv/block — nothing is pre-rendered in
// the classifier.
//
// `method: &'a str` on `BangStrip` is the *stripped* name
// (`"destroy"` for a `"destroy!"` input) since the emitters that
// strip all want that form; Crystal — which keeps the bang — bypasses
// this variant and renders through its own path.

/// Classified shape of a controller-body `Send`. `None` from
/// `classify_controller_send` means "fall through to the emitter's
/// normal Send rendering."
#[derive(Debug)]
pub enum SendKind<'a> {
    // HTTP surface — bare calls with no receiver, no block.
    /// `render(args...)` bare.
    Render { args: &'a [Expr] },
    /// `redirect_to(args...)` bare.
    RedirectTo { args: &'a [Expr] },
    /// `head(status)` bare.
    Head { args: &'a [Expr] },

    // respond_to + format.* routing.
    /// `respond_to do |format| body end` — `body` is the unwrapped
    /// block body (Lambda layer already peeled).
    RespondToBlock { body: &'a Expr },
    /// `format.html { body }` — `body` is the unwrapped block body.
    FormatHtml { body: &'a Expr },
    /// `format.json { … }` — contents intentionally dropped per
    /// Phase 4c's JSON-branch-is-TODO convention.
    FormatJson,

    // Params surface.
    /// Bare `params`.
    ParamsAccess,
    /// `params.expect(args...)`.
    ParamsExpect { args: &'a [Expr] },
    /// `params[key]`.
    ParamsIndex { key: &'a Expr },

    // Model class methods.
    /// `Model.new` / `Model.new(anything)` — args dropped by every
    /// emitter (generated models have no keyword/positional ctor).
    ModelNew { class: Symbol },
    /// `Model.find(id)` — the class method, returning a nullable.
    /// Each emitter appends its own unwrap flavour.
    ModelFind { class: Symbol, id: &'a Expr },

    // Association / query chain shapes.
    /// `<assoc>.find(x)` / `<assoc>.build(h)` / `<assoc>.create(h)`
    /// on a non-Const receiver (the outer method's `recv` is a
    /// Send whose method name singularizes to a known model).
    /// Every emitter renders this as a zero-value of `target`.
    AssocLookup { target: Symbol, outer_method: &'a str },
    /// Unsupported query-builder chain (`.all`/`.order`/`.where`/…).
    /// Target class from chain walk; `None` when the chain's head
    /// isn't a known model.
    QueryChain { target: Option<Symbol> },

    /// Bare `*_path` / `*_url` — Rails URL helpers. No runtime.
    PathOrUrlHelper,

    /// `.destroy!` / `.save!` / `.update!` — three of four targets
    /// (Rust, Go, Elixir) strip the bang; Crystal accepts it and
    /// bypasses this variant.
    BangStrip {
        recv: &'a Expr,
        stripped_method: &'a str,
        args: &'a [Expr],
    },

    /// `x.update(...)` on a non-Const receiver — no runtime yet; all
    /// four emitters punt to a boolean stub.
    InstanceUpdate,
}

/// Classify a controller-body `Send` into a shared `SendKind` variant.
/// Returns `None` for shapes that don't match any shared pattern;
/// the caller falls through to its normal Send rendering (self-
/// dispatch, plain `recv.method(args)`, etc.).
///
/// This is the IR side of the Phase 4c shared-analysis work. The
/// four emitters each had a near-identical match-and-rewrite pass;
/// this function captures it once, and the emitters become render
/// tables over `SendKind`.
pub fn classify_controller_send<'a>(
    recv: Option<&'a Expr>,
    method: &'a str,
    args: &'a [Expr],
    block: Option<&'a Expr>,
    known_models: &[Symbol],
) -> Option<SendKind<'a>> {
    // Bare `params` — a zero-arg, no-block Send with recv=None.
    if recv.is_none() && method == "params" && args.is_empty() && block.is_none() {
        return Some(SendKind::ParamsAccess);
    }

    // `params.expect(...)` and `params[k]` — recv must match the
    // bare-`params` shape.
    if let Some(r) = recv {
        if is_params_expr(r) {
            if method == "expect" {
                return Some(SendKind::ParamsExpect { args });
            }
            if method == "[]" && !args.is_empty() {
                return Some(SendKind::ParamsIndex { key: &args[0] });
            }
        }
    }

    // `respond_to do |format| ... end`. Unwrap one Lambda layer so
    // the emitter sees the block body directly.
    if recv.is_none() && method == "respond_to" && block.is_some() {
        let body = unwrap_lambda(block.unwrap());
        return Some(SendKind::RespondToBlock { body });
    }

    // `format.html { body }` / `format.json { body }`.
    if let Some(r) = recv {
        if is_format_binding(r) {
            match method {
                "html" => {
                    if let Some(b) = block {
                        let body = unwrap_lambda(b);
                        return Some(SendKind::FormatHtml { body });
                    }
                }
                "json" => return Some(SendKind::FormatJson),
                _ => {}
            }
        }
    }

    // Bare `render` / `redirect_to` / `head`.
    if recv.is_none() && block.is_none() {
        match method {
            "render" => return Some(SendKind::Render { args }),
            "redirect_to" => return Some(SendKind::RedirectTo { args }),
            "head" => return Some(SendKind::Head { args }),
            _ => {}
        }
    }

    // Bare `*_path` / `*_url` — Rails URL helper.
    if recv.is_none()
        && block.is_none()
        && (method.ends_with("_path") || method.ends_with("_url"))
    {
        return Some(SendKind::PathOrUrlHelper);
    }

    // `Model.new` / `Model.new(...)` and `Model.find(id)` — class
    // method calls on a known model.
    if let Some(r) = recv {
        if let ExprNode::Const { path } = &*r.node {
            if let Some(class) = path.last() {
                if let Some(resolved) = known_models.iter().find(|m| *m == class) {
                    match method {
                        "new" => {
                            return Some(SendKind::ModelNew {
                                class: resolved.clone(),
                            });
                        }
                        "find" => {
                            if let Some(id) = args.first() {
                                return Some(SendKind::ModelFind {
                                    class: resolved.clone(),
                                    id,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Query-builder chains — `all`/`includes`/`order`/`where`/... on
    // anything. Target model is the chain's Const head.
    if is_query_builder_method(method) {
        let target = recv.and_then(|r| chain_target_class(r, known_models));
        return Some(SendKind::QueryChain { target });
    }

    // `<assoc>.find(x)` / `.build(h)` / `.create(h)` on a Send recv
    // whose inner method singularizes to a known model.
    if matches!(method, "find" | "build" | "create") {
        if let Some(r) = recv {
            if let ExprNode::Send {
                recv: Some(_),
                method: assoc_method,
                args: inner_args,
                ..
            } = &*r.node
            {
                if inner_args.is_empty() {
                    if let Some(target) =
                        singularize_to_model(assoc_method.as_str(), known_models)
                    {
                        return Some(SendKind::AssocLookup {
                            target,
                            outer_method: method,
                        });
                    }
                }
            }
        }
    }

    // `.destroy!` / `.save!` / `.update!` — three of four emitters
    // strip the bang. Crystal bypasses this variant and renders the
    // bang-suffixed name directly.
    if let Some(r) = recv {
        if method == "destroy!" || method == "save!" || method == "update!" {
            let stripped = &method[..method.len() - 1];
            return Some(SendKind::BangStrip {
                recv: r,
                stripped_method: stripped,
                args,
            });
        }
    }

    // `x.update(...)` on a non-Const receiver.
    if method == "update" {
        if let Some(r) = recv {
            if !matches!(&*r.node, ExprNode::Const { .. }) {
                return Some(SendKind::InstanceUpdate);
            }
        }
    }

    None
}

/// Peel one `ExprNode::Lambda` layer — Ruby `do ... end` / `{ ... }`
/// ingests as a `Lambda` in the IR, but for emit purposes each block
/// is rendered as its body's statements, not as a lambda.
fn unwrap_lambda(e: &Expr) -> &Expr {
    match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    }
}

// Pass-2 shared helpers ------------------------------------------------
//
// Every target emitter's pass-2 controller rendering needs the same
// four pieces of analysis: the resource name (singular snake_case
// from the controller class), whether the controller is a nested
// child of another resource, the list of fields its `_params`
// helper permits, and a fallback list when the helper can't be
// parsed. Lifted from six near-identical per-target copies; the
// only variation was Rust's app-driven route-table walk for nested
// parents (vs. hardcoded "comment → article" in the other five) —
// we keep the Rust shape as canonical since it scales.

/// `ArticlesController` → `"article"`. `ApplicationController` →
/// `"application"`. Used to look up the `<resource>_params` helper
/// and to build default redirect paths.
pub fn resource_from_controller_name(class_name: &str) -> String {
    let trimmed = class_name.strip_suffix("Controller").unwrap_or(class_name);
    naming::singularize(&naming::snake_case(trimmed))
}

/// One nested-parent entry, carrying both forms for use in route
/// helpers and typed destinations. `singular` is the Ruby-style
/// singular ("article"); `plural` is the route segment
/// ("articles").
#[derive(Clone, Debug)]
pub struct NestedParent {
    pub singular: String,
    pub plural: String,
}

/// Walk the route table looking for a `resources :plural do resources
/// :child ... end` shape where `child` matches this controller's
/// resource. Returns the parent's (singular, plural) pair so the
/// emitter can emit `parent_id` path params and parent-redirects.
///
/// Recurses into nested blocks so deeper-than-two-level nesting
/// still resolves correctly.
pub fn find_nested_parent(app: &App, controller_class_name: &str) -> Option<NestedParent> {
    let resource = resource_from_controller_name(controller_class_name);
    let child_plural = naming::pluralize_snake(&naming::camelize(&resource));
    find_nested_parent_in(&app.routes.entries, &child_plural)
}

fn find_nested_parent_in(
    entries: &[RouteSpec],
    child_plural: &str,
) -> Option<NestedParent> {
    for entry in entries {
        if let RouteSpec::Resources { name, nested, .. } = entry {
            for child in nested {
                if let RouteSpec::Resources { name: child_name, .. } = child {
                    if child_name.as_str() == child_plural {
                        let parent_singular =
                            naming::singularize_camelize(name.as_str()).to_lowercase();
                        return Some(NestedParent {
                            singular: parent_singular,
                            plural: name.as_str().to_string(),
                        });
                    }
                }
            }
            if let Some(p) = find_nested_parent_in(nested, child_plural) {
                return Some(p);
            }
        }
    }
    None
}

/// Dig the `<resource>_params` private helper out of the controller
/// and extract the permitted field names from its
/// `params.expect(scope: [:field1, :field2])` body. Returns `None`
/// when the helper doesn't exist or the body doesn't match the
/// canonical Rails scaffold shape — callers fall back to
/// `default_permitted_fields`.
pub fn permitted_fields_for(
    controller: &Controller,
    resource: &str,
) -> Option<Vec<String>> {
    let helper_name = format!("{resource}_params");
    let action = controller.body.iter().find_map(|item| match item {
        ControllerBodyItem::Action { action, .. }
            if action.name.as_str() == helper_name =>
        {
            Some(action)
        }
        _ => None,
    })?;
    extract_permitted_from_expr(&action.body)
}

/// Walk an expression looking for a `params.expect(<scope>: [:f1,
/// :f2])` call and return the field name list. Recurses into Seqs
/// so a helper with a guard or local first still resolves.
pub fn extract_permitted_from_expr(expr: &Expr) -> Option<Vec<String>> {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*expr.node {
        if method.as_str() == "expect" && is_params_expr(r) {
            if let Some(arg) = args.first() {
                if let ExprNode::Hash { entries, .. } = &*arg.node {
                    if let Some((_, value)) = entries.first() {
                        if let ExprNode::Array { elements, .. } = &*value.node {
                            let fields: Vec<String> = elements
                                .iter()
                                .filter_map(|e| match &*e.node {
                                    ExprNode::Lit {
                                        value: Literal::Sym { value },
                                    } => Some(value.as_str().to_string()),
                                    _ => None,
                                })
                                .collect();
                            if !fields.is_empty() {
                                return Some(fields);
                            }
                        }
                    }
                }
            }
        }
    }
    if let ExprNode::Seq { exprs } = &*expr.node {
        for e in exprs {
            if let Some(v) = extract_permitted_from_expr(e) {
                return Some(v);
            }
        }
    }
    None
}

/// The seven standard Rails scaffold actions plus an Unknown fallback
/// for anything the template-per-action pipeline doesn't model.
/// Emitters dispatch on this to pick a render template; the per-
/// target code shrinks to "render this variant."
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Index,
    Show,
    New,
    Edit,
    Create,
    Update,
    Destroy,
    /// Any custom action — emitters render as a 501 stub keyed off
    /// `LoweredAction::name`.
    Unknown,
}

impl ActionKind {
    fn from_name(name: &str) -> Self {
        match name {
            "index" => Self::Index,
            "show" => Self::Show,
            "new" => Self::New,
            "edit" => Self::Edit,
            "create" => Self::Create,
            "update" => Self::Update,
            "destroy" => Self::Destroy,
            _ => Self::Unknown,
        }
    }
}

/// Target-neutral view of one action's emit-relevant inputs. Every
/// pass-2 emitter needed the same six facts (name, resource, model
/// class, whether the model exists, nested parent, permitted
/// fields) — lifting them into a single struct is the forcing
/// function for collapsing 42 near-identical per-target functions
/// down to six render tables.
#[derive(Clone, Debug)]
pub struct LoweredAction {
    pub kind: ActionKind,
    /// The action's declared name in Ruby (`"index"`, `"create"`,
    /// and also arbitrary custom-action names when `kind ==
    /// Unknown`). Emitters can derive their target-specific handler
    /// names (`PostsIndex`, `articles/index`, etc.) from this plus
    /// the controller class.
    pub name: String,
    /// Singular snake-case resource name (`"article"`). Used to
    /// key form-body params (`"article[title]"`) and to derive
    /// route helpers.
    pub resource: String,
    /// PascalCase model class (`"Article"`). Empty when
    /// `has_model` is false.
    pub model_class: String,
    /// Whether the resource maps to a known model in this app.
    /// Emitters gate the DB-touching body on this; an
    /// `ApplicationController`'s actions lower with
    /// `has_model = false`.
    pub has_model: bool,
    /// The parent resource when this controller is nested under
    /// another (`comment → article`).
    pub parent: Option<NestedParent>,
    /// Field names to pick out of form-body params during
    /// create/update.
    pub permitted: Vec<String>,
}

/// Build a `LoweredAction` from the inputs every pass-2 emitter
/// already computed at the controller-file level. Cheap to
/// construct — essentially just a tagged bundle.
pub fn lower_action(
    name: &str,
    resource: &str,
    model_class: &str,
    has_model: bool,
    parent: Option<&NestedParent>,
    permitted: &[String],
) -> LoweredAction {
    LoweredAction {
        kind: ActionKind::from_name(name),
        name: name.to_string(),
        resource: resource.to_string(),
        model_class: model_class.to_string(),
        has_model,
        parent: parent.cloned(),
        permitted: permitted.to_vec(),
    }
}

// -- Pre-emit body normalization -----------------------------------
//
// Lowering passes that reshape an action's body `Expr` into a form
// every target emitter can walk without per-target special cases.
// These codify Rails semantics (implicit render, before_action
// callbacks, respond_to dispatch, strong_params) once, so emitters
// see a normalized body and stay thin.

/// Prepend the body of each applicable `before_action` callback to
/// `body`. A filter applies when its `only:` list contains
/// `action_name` (or it has no `only:` and no `except:` match —
/// i.e. it applies to every action). Multiple applicable filters
/// prepend in declaration order.
///
/// Filters whose target isn't a private method in this controller
/// (e.g. `authenticate_user` inherited from ApplicationController or
/// a concern) are dropped with no inlining — matches the current
/// emit convention of ignoring inherited callbacks, which will
/// change when the concern-resolution pass arrives.
///
/// Target-neutral. Returns a new `Expr`; the input body is untouched.
pub fn resolve_before_actions(
    controller: &Controller,
    action_name: &str,
    body: &Expr,
) -> Expr {
    let applicable: Vec<&Filter> = controller
        .filters()
        .filter(|f| matches!(f.kind, FilterKind::Before))
        .filter(|f| filter_applies(f, action_name))
        .collect();
    if applicable.is_empty() {
        return body.clone();
    }
    // Look up each filter's target in the controller's own private
    // methods (stored as `Action`s after the `PrivateMarker`).
    // Targets that don't resolve (inherited callbacks) are silently
    // dropped.
    let mut prepend: Vec<Expr> = Vec::new();
    for f in applicable {
        if let Some(method) = controller.actions().find(|a| a.name == f.target) {
            prepend.push(method.body.clone());
        }
    }
    if prepend.is_empty() {
        return body.clone();
    }
    match &*body.node {
        ExprNode::Seq { exprs } => {
            prepend.extend(exprs.iter().cloned());
        }
        _ => prepend.push(body.clone()),
    }
    Expr::new(body.span, ExprNode::Seq { exprs: prepend })
}

/// True when `filter` applies to `action_name` given its `only:` /
/// `except:` restrictions. Mirrors Rails' semantics: `only` is a
/// whitelist, `except` is a blacklist, neither means all actions.
fn filter_applies(filter: &Filter, action_name: &str) -> bool {
    if !filter.only.is_empty() {
        return filter.only.iter().any(|s| s.as_str() == action_name);
    }
    if !filter.except.is_empty() {
        return !filter.except.iter().any(|s| s.as_str() == action_name);
    }
    true
}

/// Flatten every `respond_to do |format| ... end` block in `expr`
/// into just its HTML branch: each `format.html { body }` is
/// replaced with its block body contents, and each `format.json
/// { … }` is dropped. Mirrors the Phase-4c convention already baked
/// into `SendKind::FormatJson` — JSON branches are deferred to a
/// later phase, so flattening to HTML-only is target-neutral and
/// lossless for the HTTP-HTML paths every emitter targets today.
///
/// Handles both scaffold shapes:
///   - Simple:    `respond_to { format.html { a }; format.json { b } }` → `a`
///   - Branched:  `respond_to { if c; format.html { a1 }; format.json { b1 }
///                              else;  format.html { a2 }; format.json { b2 } end }`
///                 → `if c; a1 else a2 end`
///
/// Walks recursively — nested `respond_to` calls (rare) flatten
/// bottom-up, and non-respond_to sub-expressions pass through their
/// structural variants so anything already at the top level is
/// preserved.
pub fn unwrap_respond_to(expr: &Expr) -> Expr {
    // Top-level `respond_to` with a block — replace the whole Send
    // with its flattened HTML-only body. This short-circuits the
    // structural recursion so we don't re-enter the respond_to's
    // Send/Lambda children via the generic path.
    if let ExprNode::Send { recv: None, method, block: Some(block), .. } = &*expr.node {
        if method.as_str() == "respond_to" {
            let lambda_body = unwrap_lambda(block);
            return flatten_respond_to_body(lambda_body);
        }
    }
    let new_node = match &*expr.node {
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(unwrap_respond_to).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: unwrap_respond_to(cond),
            then_branch: unwrap_respond_to(then_branch),
            else_branch: unwrap_respond_to(else_branch),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(unwrap_respond_to),
            method: method.clone(),
            args: args.iter().map(unwrap_respond_to).collect(),
            block: block.as_ref().map(unwrap_respond_to),
            parenthesized: *parenthesized,
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: unwrap_respond_to(left),
            right: unwrap_respond_to(right),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: unwrap_respond_to(body),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: unwrap_respond_to(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: unwrap_respond_to(recv),
                    index: unwrap_respond_to(index),
                },
                other => other.clone(),
            };
            ExprNode::Assign {
                target: new_target,
                value: unwrap_respond_to(value),
            }
        }
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(unwrap_respond_to).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (unwrap_respond_to(k), unwrap_respond_to(v)))
                .collect(),
            braced: *braced,
        },
        // Literal, Const, Var, Ivar, Apply, Case, Yield, Raise,
        // RescueModifier, StringInterp, Let — no respond_to inside
        // today's fixtures; clone-verbatim. If future fixtures nest
        // respond_to inside these variants the recursion extends
        // here.
        other => other.clone(),
    };
    Expr {
        span: expr.span,
        node: Box::new(new_node),
        ty: expr.ty.clone(),
        effects: expr.effects.clone(),
        leading_blank_line: expr.leading_blank_line,
    }
}

/// Flatten the immediate body of a `respond_to` block. Recognized
/// shapes at this level are `Seq` (the `format.html/.json` pair) and
/// `If` (conditional branching to different format pairs); anything
/// else is handled via `format_stmt_to_html_only` directly.
fn flatten_respond_to_body(body: &Expr) -> Expr {
    match &*body.node {
        ExprNode::Seq { exprs } => {
            let kept: Vec<Expr> =
                exprs.iter().filter_map(format_stmt_to_html_only).collect();
            // Single-element Seq → unwrap so the downstream walker
            // sees an ordinary Send instead of a Seq-of-one.
            match kept.len() {
                0 => Expr::new(body.span, ExprNode::Seq { exprs: vec![] }),
                1 => kept.into_iter().next().unwrap(),
                _ => Expr::new(body.span, ExprNode::Seq { exprs: kept }),
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => Expr::new(
            body.span,
            ExprNode::If {
                cond: unwrap_respond_to(cond),
                then_branch: flatten_respond_to_body(then_branch),
                else_branch: flatten_respond_to_body(else_branch),
            },
        ),
        // A single expression at respond_to-body scope — either a
        // lone `format.html`/`format.json`, or some unrelated shape
        // the pass leaves to the generic walker.
        _ => format_stmt_to_html_only(body).unwrap_or_else(|| unwrap_respond_to(body)),
    }
}

/// Map one statement inside a respond_to body:
/// - `format.html { body }` → `Some(body)` (the block contents are lifted out)
/// - `format.html` (no block) → `Some(empty Seq)` (the header-only form)
/// - `format.json { … }` → `None` (drop)
/// - anything else → `Some(unwrap_respond_to(e))` (keep, recursively flattened)
fn format_stmt_to_html_only(e: &Expr) -> Option<Expr> {
    if let ExprNode::Send { recv: Some(recv), method, block, .. } = &*e.node {
        if is_format_binding(recv) {
            match method.as_str() {
                "html" => {
                    let content = match block.as_ref() {
                        Some(b) => unwrap_lambda(b).clone(),
                        None => Expr::new(e.span, ExprNode::Seq { exprs: vec![] }),
                    };
                    return Some(unwrap_respond_to(&content));
                }
                "json" => return None,
                _ => {}
            }
        }
    }
    Some(unwrap_respond_to(e))
}

/// Append a synthesized `render :<action_name>` Send to `body` when
/// `body` has no top-level render / redirect_to / head terminal.
/// Encodes the Rails convention that an action falling off the end
/// renders its eponymous view.
///
/// Target-neutral — every emitter walking the result sees an explicit
/// terminal that `classify_controller_send` resolves to `Render`.
/// Before this pass, each scaffold template synthesized the terminal
/// ad-hoc at emit time; after, the walker path needs no special case.
pub fn synthesize_implicit_render(body: &Expr, action_name: &str) -> Expr {
    if has_toplevel_terminal(body) {
        return body.clone();
    }
    let render = render_symbol_send(action_name, body.span);
    append_statement(body, render)
}

/// True when `body` is guaranteed to hit a response-terminal
/// (`render` / `redirect_to` / `head` / `respond_to`) at its top
/// level — including every branch of the final if/else, since both
/// branches must terminate for the action to have a response. A
/// `respond_to` block counts as terminal because the emitter's
/// SendKind render table expands it into per-format terminals.
pub fn has_toplevel_terminal(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.last().map_or(false, has_toplevel_terminal),
        ExprNode::Send { recv: None, method, block, .. } => {
            matches!(method.as_str(), "render" | "redirect_to" | "head")
                || (method.as_str() == "respond_to" && block.is_some())
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            has_toplevel_terminal(then_branch) && has_toplevel_terminal(else_branch)
        }
        _ => false,
    }
}

/// Build a synthetic `render :<name>` Send with the given span.
/// Used by `synthesize_implicit_render`; span is inherited from the
/// containing body so diagnostics / effect annotations point at a
/// meaningful location rather than a free-floating synthetic span.
fn render_symbol_send(action_name: &str, span: crate::span::Span) -> Expr {
    let sym = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Sym { value: Symbol::from(action_name) },
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("render"),
            args: vec![sym],
            block: None,
            parenthesized: false,
        },
    )
}

/// Append `tail` as the final statement of `body`. If `body` is
/// already a `Seq`, the result is a `Seq` with one more element;
/// otherwise the result wraps both in a new `Seq`.
fn append_statement(body: &Expr, tail: Expr) -> Expr {
    let mut exprs = match &*body.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![body.clone()],
    };
    exprs.push(tail);
    Expr::new(body.span, ExprNode::Seq { exprs })
}

/// Apply the full pre-emit normalization pipeline to an action
/// body — the canonical three-pass sequence every target emitter
/// runs verbatim before walking. Returns a new `Expr`; the input
/// body is untouched.
///
///   1. `resolve_before_actions` — inline `before_action` callback
///      bodies into each action that uses them.
///   2. `unwrap_respond_to` — flatten `respond_to { format.html {…}
///      format.json {…} }` blocks to just their HTML branch.
///   3. `synthesize_implicit_render` — append `render :<action>`
///      when the body has no explicit response terminal.
///
/// Per-target ivar/params rewrites happen AFTER this pipeline
/// (e.g. TS's `rewrite_for_controller`), since the rewrite shape
/// differs between targets (JS-friendly `context.params.k` vs
/// Rust's axum-extractor locals).
pub fn normalize_action_body(
    controller: &Controller,
    action_name: &str,
    body: &Expr,
) -> Expr {
    let with_callbacks = resolve_before_actions(controller, action_name, body);
    let flattened = unwrap_respond_to(&with_callbacks);
    synthesize_implicit_render(&flattened, action_name)
}

/// True when `body` is an empty `Seq` or a `nil` literal — the two
/// shapes every walker needs to recognize so `if cond; A; end` with
/// no else-branch doesn't emit a spurious empty `else { }` block.
pub fn is_empty_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*body.node, ExprNode::Lit { value: Literal::Nil })
}

// -- Pattern detection helpers (target-neutral) -------------------
//
// Every emitter's controller walker needs to recognize the same
// Rails scaffold shapes: `Model.new(post_params)`, `x.update(
// post_params)`, render / redirect_to with a `status:` kwarg.
// The matching is pure IR structure; the per-target rendering
// (how to express a field assign, how to emit a status code)
// stays in the emitter.

/// True when `expr` is a zero-arg, zero-block, no-receiver call to
/// the resource's strong-params helper — e.g. `post_params` when
/// the resource is `post`, `article_params` when it's `article`.
/// Rails convention: every scaffold controller defines a
/// `<resource>_params` private method that returns the permitted
/// form fields.
pub fn is_resource_params_call(expr: &Expr, resource: &str) -> bool {
    let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else {
        return false;
    };
    if !args.is_empty() || block.is_some() {
        return false;
    }
    let expected = format!("{resource}_params");
    method.as_str() == expected
}

/// Recognize a Create-scaffold instantiation: either
///   `Model.new(<resource>_params)`                (top-level)
/// or
///   `@parent.<assoc>.build(<resource>_params)`    (nested resource)
/// where the target class resolves to one of the app's known
/// models. Returns the target class symbol so the walker can emit
/// `new Class()` plus per-target per-field assigns. Used by every
/// emitter's Assign handler — the strong-params expansion shape
/// differs per target but the recognition is target-neutral.
pub fn model_new_with_strong_params(
    value: &Expr,
    known_models: &[Symbol],
    resource: &str,
) -> Option<Symbol> {
    let ExprNode::Send { recv, method, args, .. } = &*value.node else {
        return None;
    };
    if args.len() != 1 || !is_resource_params_call(&args[0], resource) {
        return None;
    }
    let r = recv.as_ref()?;
    // Top-level: `Model.new(...)` on a Const receiver resolving to
    // a known model.
    if method.as_str() == "new" {
        if let ExprNode::Const { path } = &*r.node {
            let class = path.last()?;
            known_models.iter().find(|m| *m == class)?;
            return Some(class.clone());
        }
    }
    // Nested: `<parent>.<assoc>.build(...)` — the inner Send's
    // method (a plural association like `comments`) singularizes
    // to a known model (`Comment`).
    if method.as_str() == "build" {
        if let ExprNode::Send {
            recv: Some(_),
            method: assoc_method,
            args: inner_args,
            ..
        } = &*r.node
        {
            if inner_args.is_empty() {
                if let Some(target) =
                    singularize_to_model(assoc_method.as_str(), known_models)
                {
                    return Some(target);
                }
            }
        }
    }
    None
}

/// Recognize the Update-scaffold pattern: `<recv>.update(
/// <resource>_params)` on a non-Const receiver (a local, not a
/// class method). Returns the receiver `Expr` so the walker can
/// emit `<recv>.save` plus hoisted per-field conditional assigns.
pub fn update_with_strong_params<'a>(
    cond: &'a Expr,
    resource: &str,
) -> Option<&'a Expr> {
    let ExprNode::Send { recv, method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "update" || args.len() != 1 {
        return None;
    }
    let r = recv.as_ref()?;
    if matches!(&*r.node, ExprNode::Const { .. }) {
        return None;
    }
    if !is_resource_params_call(&args[0], resource) {
        return None;
    }
    Some(r)
}

/// Map a Rails status symbol (`:see_other`, `:unprocessable_entity`)
/// to its HTTP numeric code. Covers the codes the scaffold blog
/// templates use; unknown symbols fall back to 500.
pub fn status_sym_to_code(sym: &str) -> u16 {
    match sym {
        "ok" => 200,
        "created" => 201,
        "no_content" => 204,
        "see_other" => 303,
        "not_modified" => 304,
        "bad_request" => 400,
        "unauthorized" => 401,
        "not_found" => 404,
        "unprocessable_entity" => 422,
        _ => 500,
    }
}

/// Walk `render` / `redirect_to` keyword args for a `status:` key
/// and return its numeric string. Accepts symbol values
/// (`status: :see_other`) and integer literals (`status: 303`).
/// Returns `None` when no status is supplied, so callers can pick
/// their own default.
pub fn extract_status_from_kwargs(args: &[Expr]) -> Option<u16> {
    for arg in args {
        let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                continue;
            };
            if key.as_str() != "status" {
                continue;
            }
            match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value: s } } => {
                    return Some(status_sym_to_code(s.as_str()));
                }
                ExprNode::Lit { value: Literal::Int { value: n } } => {
                    return Some(*n as u16);
                }
                _ => return None,
            }
        }
    }
    None
}

/// Fallback permitted-field list when the `<resource>_params` helper
/// isn't recognizable. Returns the model's non-id, non-timestamp,
/// non-foreign-key attributes — a safe default that matches what
/// the Rails scaffold generator would produce.
pub fn default_permitted_fields(app: &App, model_class: &str) -> Vec<String> {
    let Some(model) = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == model_class)
    else {
        return Vec::new();
    };
    model
        .attributes
        .fields
        .keys()
        .map(|k| k.as_str().to_string())
        .filter(|name| {
            name != "id"
                && name != "created_at"
                && name != "updated_at"
                && !name.ends_with("_id")
        })
        .collect()
}
