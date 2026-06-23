//! Body-typer: the Rails-agnostic core of type inference.
//!
//! Given an [`Expr`] + a [`Ctx`] + a method dispatch table (a
//! `HashMap<ClassId, ClassInfo>`), walks the tree and annotates each
//! node's `ty` field. This module knows nothing about schemas,
//! controllers, `before_action` chains, or any other Rails dialect —
//! it's pure "Ruby body type inference against known signatures."
//!
//! The Rails-aware [`super::Analyzer`] pre-computes the dispatch
//! table from `App.models` and then threads a [`BodyTyper`] over each
//! method body, action body, and view body.
//!
//! Runtime-extraction code (src/runtime_src.rs) uses the same
//! [`BodyTyper`] with a simpler dispatch table (no user classes, just
//! the primitive method tables).

use std::collections::HashMap;

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TyVar};
use crate::ty::{Row, Ty};

mod diagnostic;
mod narrowing;
mod send;

/// Recursion context — what `self` is, what locals/ivars are in scope.
/// Immutable during descent; clone to enter a new scope (Let body,
/// block body, Seq walk with new ivar/local bindings).
#[derive(Clone, Default)]
pub struct Ctx {
    pub self_ty: Option<Ty>,
    /// Ivar bindings observed as a `Seq` walks its statements in order.
    /// `@post = Post.find(...)` in stmt 1 lets `@post.destroy` in stmt 2
    /// dispatch correctly.
    pub ivar_bindings: HashMap<Symbol, Ty>,
    /// Local-variable bindings in the current scope: let-bound names,
    /// assignments accumulated through a `Seq`, and block parameters
    /// seeded from a receiver-aware dispatch.
    pub local_bindings: HashMap<Symbol, Ty>,
    /// Module/class-level constants: `STATUS_CODES = { ok: 200, ... }.freeze`.
    /// Populated by the registry-builder from parsed module bodies; read
    /// by the `ExprNode::Const` arm so subsequent dispatch on the constant
    /// (`STATUS_CODES.fetch(...)`) lands in the right primitive method
    /// table instead of falling through to the user-class registry.
    pub constants: HashMap<Symbol, Ty>,
    /// When set, the body-typer writes back `Some(SelfRef)` on the
    /// recv slot of bare Sends that resolve via `self_ty`'s dispatch
    /// table. Off by default — opt-in per call site so targets that
    /// model self differently (e.g. Crystal's module-of-self-actions
    /// controller shape, Python/Elixir/Go's similar) aren't forced to
    /// recognize a new IR shape across every existing per-target
    /// rewriter at once. Enable for paths that consume the
    /// LibraryClass directly as instance methods (runtime/ruby/* via
    /// parse_library_with_rbs, view_to_library's body-typer pass,
    /// the upcoming TS controller_thin path).
    pub annotate_self_dispatch: bool,
    /// Set when typing a view/layout body. In that context `yield` renders
    /// content to a String (bare `yield` → the layout's `body` String param;
    /// `yield :slot` → `ViewHelpers.get_slot` → String — see
    /// `lower/view_to_library/partial.rs::emit_yield`), so the Yield arm types
    /// it `String` instead of the generic `Untyped`. Off elsewhere, where a
    /// block's return type isn't tracked through the method signature.
    pub in_view: bool,
}

/// User-class dispatch data: table name (if any), instance shape,
/// class/instance method tables. Built by [`super::Analyzer`] from
/// Rails schema + conventions; the body-typer reads it.
#[derive(Default, Clone)]
pub struct ClassInfo {
    /// If this class maps to a database table, which one.
    pub table: Option<crate::ident::TableRef>,
    /// Instance-state shape (columns + attr_accessor).
    pub attributes: Row,
    /// Methods callable on the class itself: `Post.all`, `Post.find(id)`.
    pub class_methods: HashMap<Symbol, Ty>,
    /// Methods callable on an instance: `post.title`, `post.destroy`.
    pub instance_methods: HashMap<Symbol, Ty>,
    /// AccessorKind per method — lets the body-typer flag Method
    /// dispatches so emitters add parens. AttributeReader/Writer
    /// dispatches keep `parenthesized` as ingested. Default `Method`
    /// is assumed when a name isn't present.
    pub class_method_kinds: HashMap<Symbol, crate::dialect::AccessorKind>,
    pub instance_method_kinds: HashMap<Symbol, crate::dialect::AccessorKind>,
    /// Parent class for inheritance lookups. The force-parens check
    /// in the Send arm walks this chain so methods inherited from
    /// (e.g.) ActiveRecord::Base resolve when called on a subclass
    /// (`new Article(...).save` looks up `save` on Article first,
    /// then ApplicationRecord, then Base — finding it as
    /// `AccessorKind::Method` and forcing the parens). Class lookups
    /// use the same `ClassId` shape the registry keys use.
    pub parent: Option<crate::ident::ClassId>,
    /// Modules mixed in via `include` (e.g. a controller's
    /// `include IntervalHelper`). A mixed-in module's instance methods
    /// become instance methods of the includer, so dispatch consults
    /// these (their own instance_methods in the registry) after the
    /// class's own methods and before walking the parent. Empty for
    /// most classes.
    pub includes: Vec<crate::ident::ClassId>,
}

/// Resolve a single-segment Const ref (like `Const { path:
/// ["HashWithIndifferentAccess"] }` from app source) to a fully-
/// qualified ClassId by walking the class registry. Returns the
/// fully-qualified `Symbol` when exactly one registry key matches —
/// "matches" meaning the key has `::` separator(s) AND the last
/// segment equals `name`. App-class single-segment keys (`Article`,
/// `Comment`) don't match (no `::`), so they stay bare. Multiple
/// matches (rare — would be e.g. `ActiveRecord::Base` and
/// `ActionController::Base`) leave the ref bare too — body-typer
/// can't disambiguate without lexical scope, and the bare form
/// still types via the registry's last-segment alias entry.
fn expand_bare_const(
    name: &Symbol,
    classes: &HashMap<ClassId, ClassInfo>,
) -> Option<Symbol> {
    let target = name.as_str();
    let suffix = format!("::{target}");
    let mut found: Option<&str> = None;
    for key in classes.keys() {
        let raw = key.0.as_str();
        if raw.contains("::") && raw.ends_with(&suffix) {
            if found.is_some() {
                return None; // ambiguous
            }
            found = Some(raw);
        }
    }
    found.map(Symbol::from)
}

/// Reusable body-type walker. Holds a borrow of the dispatch table so
/// repeated `analyze_expr` calls reuse the same lookup structures
/// without cloning.
pub struct BodyTyper<'a> {
    classes: &'a HashMap<ClassId, ClassInfo>,
}

impl<'a> BodyTyper<'a> {
    /// Submodule accessor for the dispatch table. `classes` itself
    /// stays private to this module; `send.rs` reaches it here.
    pub(super) fn classes(&self) -> &'a HashMap<ClassId, ClassInfo> {
        self.classes
    }
}

impl<'a> BodyTyper<'a> {
    pub fn new(classes: &'a HashMap<ClassId, ClassInfo>) -> Self {
        Self { classes }
    }

    /// Analyze an expression: compute its type, populate `expr.ty`,
    /// return the computed type. Recurses into sub-expressions, which
    /// in turn get their `ty` populated. After typing, runs a
    /// diagnostic-detection pass on the node to flag sites the body-
    /// typer recognizes as user errors (Incompatible `+`, …). The
    /// annotation rides with the IR so emitters can render a runtime
    /// raise-equivalent without re-classifying.
    pub fn analyze_expr(&self, expr: &mut Expr, ctx: &Ctx) -> Ty {
        let ty = self.compute(expr, ctx);
        expr.ty = Some(ty.clone());
        diagnostic::detect_diagnostic(expr);
        ty
    }

    fn compute(&self, expr: &mut Expr, ctx: &Ctx) -> Ty {
        let expr_span = expr.span;
        match &mut *expr.node {
            ExprNode::Lit { value } => lit_ty(value),

            ExprNode::Const { path } => {
                let last = path.last().cloned().unwrap_or_else(|| Symbol::from("?"));
                // Module/class-level constants seeded by the registry
                // builder. `STATUS_CODES = { ok: 200, ... }` lands here
                // typed `Hash[Sym, Int]` (not `Class { STATUS_CODES }`),
                // so subsequent dispatch on `STATUS_CODES.fetch(...)`
                // resolves through hash_method.
                if let Some(ty) = ctx.constants.get(&last) {
                    return ty.clone();
                }
                // Build a Ty::Class ClassId from the path. For multi-
                // segment writes (`ActiveSupport::HashWithIndifferentAccess`)
                // use the joined path verbatim. For single-segment app-
                // source refs (`Parameters.new(...)`, `HashWithIndifferentAccess.new`),
                // try to expand bare → fully-qualified by walking the
                // class registry: if exactly one fully-qualified entry
                // (key contains `::`) ends with `::<name>`, swap in the
                // full path AND REWRITE THE IR PATH so per-target emit
                // sees the canonical form (Crystal's `Const` arm reads
                // the path field, not the type annotation; rewriting
                // makes a single emit pass produce `::ActionView::
                // ViewHelpers.dom_id(...)` from a source-bare Const).
                // App classes (`Article`, `Comment` — single-segment
                // registry keys) stay bare. Mirrors Ruby's lexical
                // lookup walking up to top-level — but driven by the
                // registry rather than the AST scope chain.
                if path.len() == 1 {
                    if let Some(qualified) = expand_bare_const(&last, self.classes()) {
                        let segments: Vec<Symbol> = qualified
                            .as_str()
                            .split("::")
                            .map(Symbol::from)
                            .collect();
                        *path = segments;
                        return Ty::Class {
                            id: ClassId(qualified),
                            args: vec![],
                        };
                    }
                }
                let joined_path = path
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("::");
                Ty::Class {
                    id: ClassId(Symbol::from(joined_path)),
                    args: vec![],
                }
            }

            ExprNode::Var { name, .. } => ctx
                .local_bindings
                .get(name)
                .cloned()
                .unwrap_or_else(unknown),

            ExprNode::Ivar { name } => {
                ctx.ivar_bindings.get(name).cloned().unwrap_or_else(unknown)
            }

            ExprNode::SelfRef => ctx.self_ty.clone().unwrap_or_else(unknown),

            ExprNode::Return { value } => {
                self.analyze_expr(value, ctx);
                // `return x` diverges at the source position — control
                // jumps out of the method, so the expression itself
                // produces no value here. Type as `Bottom` so it drops
                // out of joins (`if cond then return x else y end`
                // types as `typeof(y)`, not `typeof(x) | typeof(y)`).
                // The value's own type was already captured via
                // analyze_expr above and contributes to the method's
                // declared return-type reconciliation elsewhere.
                Ty::Bottom
            }

            ExprNode::Super { args } => {
                if let Some(args) = args {
                    for a in args.iter_mut() {
                        self.analyze_expr(a, ctx);
                    }
                }
                // `super` invokes the parent's same-name method;
                // tracking which method we're inside (and looking up
                // its parent in the registry) is future work. Until
                // then, type as `Untyped` — `super` is an
                // intentional jump out of the typed envelope, similar
                // in spirit to RBS's `untyped` declaration. Distinct
                // from `Var` (analyzer gap) so the gradual diagnostic
                // shape is right.
                Ty::Untyped
            }

            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                self.analyze_expr(body, ctx);
                for rc in rescues.iter_mut() {
                    for c in rc.classes.iter_mut() {
                        self.analyze_expr(c, ctx);
                    }
                    self.analyze_expr(&mut rc.body, ctx);
                }
                if let Some(e) = else_branch {
                    self.analyze_expr(e, ctx);
                }
                if let Some(e) = ensure {
                    self.analyze_expr(e, ctx);
                }
                // Union of body type and rescue body types; approximate
                // as body's type for now.
                body.ty.clone().unwrap_or_else(unknown)
            }

            ExprNode::Hash { entries, .. } => {
                // Empty literals can carry a pre-stamped expected type
                // (set by `Assign`'s `propagate_expected_to_empty_container`
                // when the LHS is an Ivar/Var with a declared Hash type).
                // Honor it instead of falling back to `Hash<Var, Var>`
                // — strict targets need the concrete shape, and the
                // surrounding declaration already constrains the type.
                if entries.is_empty() {
                    if let Some(t @ Ty::Hash { .. }) = expr.ty.as_ref() {
                        return t.clone();
                    }
                }
                let mut key_ty: Option<Ty> = None;
                let mut value_ty: Option<Ty> = None;
                for (k, v) in entries.iter_mut() {
                    let kt = self.analyze_expr(k, ctx);
                    let vt = self.analyze_expr(v, ctx);
                    key_ty = Some(match key_ty.take() {
                        Some(prev) => union_of(prev, kt),
                        None => kt,
                    });
                    value_ty = Some(match value_ty.take() {
                        Some(prev) => union_of(prev, vt),
                        None => vt,
                    });
                }
                Ty::Hash {
                    key: Box::new(key_ty.unwrap_or_else(unknown)),
                    value: Box::new(value_ty.unwrap_or_else(unknown)),
                }
            }

            ExprNode::Array { elements, .. } => {
                // Mirror the empty-Hash branch above: a pre-stamped
                // `Ty::Array<T>` from `with_ty` (or from
                // `propagate_expected_to_empty_container`) takes
                // precedence over the `Var`-defaulted inference.
                if elements.is_empty() {
                    if let Some(t @ Ty::Array { .. }) = expr.ty.as_ref() {
                        return t.clone();
                    }
                }
                let mut elem_ty: Option<Ty> = None;
                for e in elements.iter_mut() {
                    let et = self.analyze_expr(e, ctx);
                    elem_ty = Some(match elem_ty.take() {
                        Some(prev) => union_of(prev, et),
                        None => et,
                    });
                }
                Ty::Array { elem: Box::new(elem_ty.unwrap_or_else(unknown)) }
            }

            ExprNode::StringInterp { parts } => {
                for p in parts.iter_mut() {
                    if let crate::expr::InterpPart::Expr { expr } = p {
                        self.analyze_expr(expr, ctx);
                    }
                }
                Ty::Str
            }

            ExprNode::BoolOp { left, op, right, .. } => {
                let lt = self.analyze_expr(left, ctx);
                // Short-circuit narrowing: when the right arm runs,
                // the left arm has already produced a value that
                // determined the short-circuit. For `&&` the right
                // arm runs only if left was truthy; for `||` only if
                // left was falsy. If the left arm was a recognized
                // narrowing predicate (`x.is_a?(T)`, `x.nil?`, etc.),
                // type the right arm under the corresponding
                // narrowed Ctx so subsequent reads of the same var
                // see the refined type. Mirrors how the `If` arm
                // threads narrowing into its then/else branches.
                let pred = narrowing::extract_narrowing(left);
                let mut right_ctx = match (&pred, &*op) {
                    (Some(p), crate::expr::BoolOpKind::And) => {
                        narrowing::apply_narrowing(ctx, p, true)
                    }
                    (Some(p), crate::expr::BoolOpKind::Or) => {
                        narrowing::apply_narrowing(ctx, p, false)
                    }
                    _ => ctx.clone(),
                };
                // Var assignments inside `left` (the canonical case is
                // `(x = find_by(...)) && x.foo`) need to flow into
                // `right`'s scope. Without this, `x` reads as Var
                // because the Seq-level Assign-to-Var handler only
                // catches statement-position assignments, not nested
                // ones inside expressions. For `&&` the assignment
                // executed (left was truthy); for `||` it executed
                // because left was falsy — in either case the binding
                // is observable on the right side.
                collect_var_assignments_into(left, &mut right_ctx.local_bindings);
                let rt = self.analyze_expr(right, &right_ctx);
                // Short-circuit: the result is either left (if it
                // determined the short-circuit) or right — a union
                // of the two operand types. For `||`, Nil never wins
                // — if left was Nil/false, control moves to right —
                // so peel Nil from the left side before unioning.
                // Closes the common `Option<T> || default_T` shape
                // (BoolOp::Or in `content_for_get(:title) || "Real
                // Blog"`) where the result type should be `T`, not
                // `Union<Option<T>, T>`. The lt-narrowed form lets
                // the rust2 emit's Family 4 (String → &str) fire.
                let lt_peeled = if matches!(op, crate::expr::BoolOpKind::Or) {
                    narrowing::remove_nil(&lt)
                } else {
                    lt
                };
                union_of(lt_peeled, rt)
            }

            ExprNode::RescueModifier { expr, fallback } => {
                let et = self.analyze_expr(expr, ctx);
                let ft = self.analyze_expr(fallback, ctx);
                union_of(et, ft)
            }

            ExprNode::Let { name, value, body, .. } => {
                let v_ty = self.analyze_expr(value, ctx);
                let mut inner = ctx.clone();
                inner.local_bindings.insert(name.clone(), v_ty);
                self.analyze_expr(body, &inner)
            }

            ExprNode::Lambda { body, .. } => {
                let body_ty = self.analyze_expr(body, ctx);
                // Synthesize a `Fn` type from the body's type. Param
                // types aren't tracked here (they were seeded into the
                // outer Ctx by block_ctx_for from the receiver
                // signature); the lambda's *own* type just records the
                // body's return type so callers can walk past the
                // Lambda node without seeing Var. Effects default to
                // pure; full effect inference is future work.
                Ty::Fn {
                    params: Vec::new(),
                    block: None,
                    ret: Box::new(body_ty),
                    effects: crate::effect::EffectSet::pure(),
                }
            }

            ExprNode::Apply { fun, args, block } => {
                self.analyze_expr(fun, ctx);
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                if let Some(b) = block { self.analyze_expr(b, ctx); }
                unknown()
            }

            ExprNode::Send { recv, method, args, block, parenthesized } => {
                // Bare-name implicit-self Send (no receiver, no args, no
                // block) resolves to a local binding when one exists. Ruby
                // parses `x` as `self.x()` when `x` wasn't assigned earlier
                // in scope; partials receive locals at render time (not via
                // syntactic assignment), so we disambiguate here instead of
                // at ingest. Block params and let-bindings end up on the
                // same code path.
                if recv.is_none() && args.is_empty() && block.is_none() {
                    if let Some(ty) = ctx.local_bindings.get(method) {
                        return ty.clone();
                    }
                }
                // Bare-name Kernel calls (no explicit receiver). `require`,
                // `require_relative`, etc. — Ruby treats these as private
                // module methods, but the body-typer otherwise resolves
                // `recv=None` against `self_ty`, which would dispatch
                // them as instance methods on the enclosing class (and
                // miss). Catch them here before the recv_ty fall-through.
                if recv.is_none() {
                    if matches!(
                        method.as_str(),
                        "require" | "require_relative" | "load" | "autoload"
                    ) {
                        for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                        return Ty::Bool;
                    }
                }

                // Self-dispatch annotation. When `recv == None` and the
                // typer's dispatch through `self_ty` finds the method on
                // the enclosing class (instance or class methods), write
                // back `recv = Some(SelfRef)` so emitters consume already-
                // explicit self-receivers without a per-emitter rewrite
                // pass. Bare calls that don't resolve (Kernel methods,
                // undefined names, names not in the class registry) stay
                // `recv = None` — those route through each target's
                // free-function code path. Targets that don't model self
                // as an instance (e.g., Crystal's module-shape controller
                // emit) render `SelfRef` as appropriate for their shape;
                // see the per-target SelfRef arms in each emit_expr.
                let resolves_through_self = ctx.annotate_self_dispatch
                    && recv.is_none()
                    && matches!(
                        ctx.self_ty.as_ref(),
                        Some(Ty::Class { id, .. })
                            if self.classes().get(id).is_some_and(|cls|
                                cls.class_methods.contains_key(method)
                                    || cls.instance_methods.contains_key(method))
                    );
                if resolves_through_self {
                    *recv = Some(crate::expr::Expr {
                        span: expr_span,
                        node: Box::new(ExprNode::SelfRef),
                        ty: ctx.self_ty.clone(),
                        effects: crate::effect::EffectSet::default(),
                        leading_blank_line: false,
                        diagnostic: None,
                        hint: None,
                        decisions: 0,
                    });
                }

                let recv_ty = match recv.as_mut() {
                    Some(r) => Some(self.analyze_expr(r, ctx)),
                    None => ctx.self_ty.clone(),
                };
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                let block_ret = if let Some(b) = block {
                    let block_ctx = self.block_ctx_for(ctx, recv_ty.as_ref(), method, b);
                    self.analyze_expr(b, &block_ctx);
                    // The Lambda walker stores the analyzed body's type
                    // on the body expr itself. `map`/`collect`/similar
                    // use that to determine the output element type.
                    if let ExprNode::Lambda { body, .. } = &*b.node {
                        body.ty.clone()
                    } else {
                        None
                    }
                } else {
                    None
                };
                // Force `parenthesized: true` when dispatch resolves
                // to a `Method`-kind on a registered class. The TS
                // emitter's bare-recv-Send fallback omits parens when
                // `parenthesized` is false (matching Ruby's reader
                // idiom); for real methods we want `obj.save()` not
                // `obj.save`. AttributeReader/Writer dispatches keep
                // the ingested value (already false for source-style
                // reads, true if source had parens). Unresolved
                // dispatches (recv unknown, method not in registry)
                // also keep the ingested value to preserve source
                // form for round-trip-through-emit.
                if !*parenthesized {
                    if let Some(Ty::Class { id, .. }) = &recv_ty {
                        use crate::dialect::AccessorKind;
                        // Walk the parent chain so methods inherited
                        // from a base class (e.g. `save` on
                        // `ActiveRecord::Base` reached via Article →
                        // ApplicationRecord → Base) resolve. The
                        // first kind found wins; cycles are
                        // impossible by construction (Ruby class
                        // hierarchy is a DAG).
                        let mut current_id: Option<&crate::ident::ClassId> = Some(id);
                        let mut seen = 0usize;
                        let mut resolved: Option<AccessorKind> = None;
                        while let Some(cid) = current_id {
                            seen += 1;
                            if seen > 32 {
                                break; // defensive cycle break
                            }
                            let Some(cls) = self.classes().get(cid) else {
                                break;
                            };
                            if let Some(k) = cls
                                .instance_method_kinds
                                .get(method)
                                .or_else(|| cls.class_method_kinds.get(method))
                            {
                                resolved = Some(*k);
                                break;
                            }
                            current_id = cls.parent.as_ref();
                        }
                        if matches!(resolved, Some(AccessorKind::Method)) {
                            *parenthesized = true;
                        }
                    }
                }
                // Source-level kwargs (`f(a: 1, b: 2)`) parse as a
                // trailing `KeywordHash` (`kwargs: true`). Ruby
                // implicitly forwards them to either an `**opts`/named
                // parameter OR an `opts = {}` positional Hash. Strict-
                // typed targets need to commit. Resolve the method's
                // declared signature: when the last param is positional
                // (`Required`/`Optional`) with Hash type, flip the
                // kwargs Hash to `kwargs: false` so emit renders as an
                // explicit `{ ... }` literal at the positional slot.
                // Methods whose last param is `Keyword`/`KeywordRest`
                // keep `kwargs: true` so emit renders bare named args
                // (Crystal NamedTuple-friendly, TS object-spread-
                // friendly via the named-param shape).
                self.normalize_trailing_kwargs(recv_ty.as_ref(), method, args);
                self.dispatch(recv_ty.as_ref(), method, block_ret.as_ref(), args)
            }

            ExprNode::If { cond, then_branch, else_branch } => {
                self.analyze_expr(cond, ctx);
                let pred = narrowing::extract_narrowing(cond);
                // Var assignments inside `cond` (e.g.
                // `if (user = User.find_by(...)) && user.is_active?`)
                // must flow into both branches: the assignment
                // executed during cond evaluation regardless of
                // branch taken. Then-branch may further narrow via
                // truthiness; else-branch sees the raw assigned type.
                let mut cond_assigns: HashMap<Symbol, Ty> = HashMap::new();
                collect_var_assignments_into(cond, &mut cond_assigns);
                let t = match &pred {
                    Some(p) => {
                        let mut then_ctx = narrowing::apply_narrowing(ctx, p, true);
                        for (k, v) in &cond_assigns {
                            then_ctx.local_bindings.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                        self.analyze_expr(then_branch, &then_ctx)
                    }
                    None => {
                        let mut then_ctx = ctx.clone();
                        for (k, v) in &cond_assigns {
                            then_ctx.local_bindings.insert(k.clone(), v.clone());
                        }
                        self.analyze_expr(then_branch, &then_ctx)
                    }
                };
                let e = match &pred {
                    Some(p) => {
                        let mut else_ctx = narrowing::apply_narrowing(ctx, p, false);
                        for (k, v) in &cond_assigns {
                            else_ctx.local_bindings.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                        self.analyze_expr(else_branch, &else_ctx)
                    }
                    None => {
                        let mut else_ctx = ctx.clone();
                        for (k, v) in &cond_assigns {
                            else_ctx.local_bindings.insert(k.clone(), v.clone());
                        }
                        self.analyze_expr(else_branch, &else_ctx)
                    }
                };
                union_of(t, e)
            }

            ExprNode::Case { scrutinee, arms } => {
                self.analyze_expr(scrutinee, ctx);
                let mut branch_tys = Vec::new();
                for arm in arms.iter_mut() {
                    if let Some(g) = &mut arm.guard { self.analyze_expr(g, ctx); }
                    branch_tys.push(self.analyze_expr(&mut arm.body, ctx));
                }
                union_many(branch_tys)
            }

            ExprNode::Seq { exprs } => {
                // Within a Seq, walk statements in order and thread
                // bindings forward: `@post = Post.find(...)` in stmt i
                // lets stmt i+1 resolve `@post`. Same for local vars —
                // `x = post.title` binds `x` for subsequent statements.
                let mut local_ctx = ctx.clone();
                let mut last = Ty::Nil;
                for e in exprs.iter_mut() {
                    last = self.analyze_expr(e, &local_ctx);
                    if let ExprNode::Assign { target, .. } = &*e.node {
                        if let Some(ty) = e.ty.clone() {
                            match target {
                                LValue::Ivar { name } => {
                                    local_ctx.ivar_bindings.insert(name.clone(), ty);
                                }
                                LValue::Var { name, .. } => {
                                    local_ctx.local_bindings.insert(name.clone(), ty);
                                }
                                _ => {}
                            }
                        }
                    }
                    // Short-circuit compound assignment (`@x ||= y`,
                    // `@x &&= y`) threads its binding forward too — the
                    // memoization idiom `@story ||= Story.find(...)` is
                    // pervasive in controllers, and without this the
                    // following `@story.title` reads `@story` as unknown.
                    // `OpAssign`'s node type is the value-expression's
                    // type (the naive desugar), so reuse it — but union
                    // with any prior binding and skip an unknown RHS so a
                    // before_action-seeded type isn't clobbered.
                    if let ExprNode::OpAssign { target, .. } = &*e.node {
                        if let Some(ty) = e.ty.clone() {
                            if !matches!(ty, Ty::Var { .. }) {
                                match target {
                                    LValue::Ivar { name } => {
                                        let merged = match local_ctx
                                            .ivar_bindings
                                            .remove(name)
                                        {
                                            Some(prev) => union_of(prev, ty),
                                            None => ty,
                                        };
                                        local_ctx.ivar_bindings.insert(name.clone(), merged);
                                    }
                                    LValue::Var { name, .. } => {
                                        let merged = match local_ctx
                                            .local_bindings
                                            .remove(name)
                                        {
                                            Some(prev) => union_of(prev, ty),
                                            None => ty,
                                        };
                                        local_ctx.local_bindings.insert(name.clone(), merged);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    // Destructuring assignment threads each target
                    // forward: `@stories, @show_more = paginate(...)`
                    // in stmt i lets `render json: @stories` in stmt
                    // i+1 (or a nested `respond_to` block) resolve.
                    // Per-position typing via `multiassign_target_ty`;
                    // targets with no usable RHS signal are left as-is.
                    if let ExprNode::MultiAssign { targets, value } = &*e.node {
                        let rhs = value.ty.clone();
                        for (i, target) in targets.iter().enumerate() {
                            let Some(ty) = multiassign_target_ty(&rhs, i) else {
                                continue;
                            };
                            match target {
                                LValue::Ivar { name } => {
                                    local_ctx.ivar_bindings.insert(name.clone(), ty);
                                }
                                LValue::Var { name, .. } => {
                                    local_ctx.local_bindings.insert(name.clone(), ty);
                                }
                                _ => {}
                            }
                        }
                    }
                    // Container element write — `hash[k] ||= []` /
                    // `hash[k] = v`. Widen the container's value type so
                    // a following `hash[k].push x` / `.join` in the same
                    // scope reads `Array|Nil` (resolvable via union
                    // dispatch) instead of `Var|Nil`. An empty `{}` seeds
                    // the value as Var; the accumulator idiom only
                    // reveals the element type at the write. The `{}` is
                    // bound on the recv ivar/local; a computed recv
                    // (`a.b[k]`) has no binding to refine, so skip it.
                    let elem_write = match &*e.node {
                        ExprNode::Assign { target: LValue::Index { recv, .. }, value }
                        | ExprNode::OpAssign {
                            target: LValue::Index { recv, .. }, value, ..
                        } => value.ty.clone().map(|t| (recv, t)),
                        _ => None,
                    };
                    if let Some((recv, rhs)) = elem_write {
                        if !matches!(rhs, Ty::Var { .. } | Ty::Bottom) {
                            let slot = match &*recv.node {
                                ExprNode::Ivar { name } => Some((true, name.clone())),
                                ExprNode::Var { name, .. } => Some((false, name.clone())),
                                _ => None,
                            };
                            if let Some((is_ivar, name)) = slot {
                                let bindings = if is_ivar {
                                    &mut local_ctx.ivar_bindings
                                } else {
                                    &mut local_ctx.local_bindings
                                };
                                if let Some(Ty::Hash { key, value }) = bindings.get(&name) {
                                    // A Var value is the empty-literal
                                    // placeholder — replace it; otherwise
                                    // union the observed element in.
                                    let new_value = if matches!(**value, Ty::Var { .. }) {
                                        rhs
                                    } else {
                                        union_of((**value).clone(), rhs)
                                    };
                                    let widened = Ty::Hash {
                                        key: key.clone(),
                                        value: Box::new(new_value),
                                    };
                                    bindings.insert(name, widened);
                                }
                            }
                        }
                    }
                    // Diverging-then narrowing: `raise X if m.nil?` —
                    // when the then-branch always diverges (Bottom),
                    // control proceeds only via the else, so the
                    // negated narrowing applies to subsequent stmts.
                    // The TypeScript flow-narrowing analog Crystal
                    // gets natively; this gives the body-typer the
                    // same per-statement type tightening so the
                    // emitter can dispatch on the narrowed type.
                    if let ExprNode::If { cond, then_branch, else_branch } = &*e.node {
                        let then_diverges = matches!(then_branch.ty.as_ref(), Some(Ty::Bottom));
                        let else_empty = match &*else_branch.node {
                            ExprNode::Lit { value: crate::expr::Literal::Nil } => true,
                            ExprNode::Seq { exprs } => exprs.is_empty(),
                            _ => false,
                        };
                        if then_diverges && else_empty {
                            if let Some(pred) = narrowing::extract_narrowing(cond) {
                                local_ctx = narrowing::apply_narrowing(&local_ctx, &pred, false);
                            }
                        }
                    }
                }
                last
            }

            ExprNode::Assign { target, value } => {
                // Pre-propagate expected type into empty-container
                // literals: `@params = {}` where `@params : Hash[K, V]`
                // should type the empty Hash as `Hash<K, V>` rather
                // than `Hash<Var, Var>`. Without this, the strict
                // Crystal emit's `{} of K => V` default falls back to
                // `String => String` and trips against the declared
                // property type. Only fires for empty literals — non-
                // empty container literals already infer from their
                // contents.
                if let LValue::Ivar { name } = target {
                    if let Some(expected) = ctx.ivar_bindings.get(name).cloned() {
                        propagate_expected_to_empty_container(value, &expected);
                    }
                }
                let value_ty = self.analyze_expr(value, ctx);
                if let LValue::Attr { recv, .. } = target {
                    self.analyze_expr(recv, ctx);
                }
                if let LValue::Index { recv, index } = target {
                    self.analyze_expr(recv, ctx);
                    self.analyze_expr(index, ctx);
                }
                value_ty
            }

            ExprNode::OpAssign { target, value, .. } => {
                // Same recursion shape as Assign: walk target's child
                // Exprs + walk value. Short-circuit-aware type
                // narrowing (suppressing the union-widen on `||=` when
                // the target is already non-Option) is a follow-on;
                // for now the analyzer treats the result as the value
                // expression's type, which matches naive desugar.
                let value_ty = self.analyze_expr(value, ctx);
                if let LValue::Attr { recv, .. } = target {
                    self.analyze_expr(recv, ctx);
                }
                if let LValue::Index { recv, index } = target {
                    self.analyze_expr(recv, ctx);
                    self.analyze_expr(index, ctx);
                }
                value_ty
            }

            ExprNode::Yield { args } => {
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                // In a view/layout, `yield` renders content → a String (the
                // lowering turns it into the `body` String param / a
                // `ViewHelpers.get_slot` String call). Elsewhere the block's
                // return type isn't tracked through the method's signature
                // today (would require generics — `def f<T> { () -> T } -> ...`),
                // so type as Untyped: the call site signed for an opaque block
                // return, and propagating Untyped lets downstream dispatch
                // resolve cleanly instead of bottoming out at Var.
                if ctx.in_view { Ty::Str } else { Ty::Untyped }
            }

            ExprNode::Raise { value } => {
                self.analyze_expr(value, ctx);
                // `raise` diverges — control transfers up the stack,
                // the expression produces no value. `Ty::Bottom`
                // drops out of unions so `if cond then raise else x
                // end` types as `typeof(x)`.
                Ty::Bottom
            }

            ExprNode::Next { value } | ExprNode::Break { value } => {
                if let Some(v) = value { self.analyze_expr(v, ctx); }
                // `next` / `break` are divergent at the source position
                // — `next` skips to the next iteration, `break` exits
                // the enclosing iterator. `Bottom` drops out of joins
                // so `if cond then next else x end` types cleanly as
                // `typeof(x)`.
                Ty::Bottom
            }

            ExprNode::Splat { value } => {
                // Splat propagates the inner expression's type
                // unchanged; the splat itself is a structural marker
                // for the surrounding Send/Array, not a transform.
                // The Send's argument-typing logic decides whether
                // the splatted Array's element type satisfies the
                // formal parameter's variadic/rest signature.
                self.analyze_expr(value, ctx)
            }

            ExprNode::MultiAssign { targets, value } => {
                self.analyze_expr(value, ctx);
                for target in targets.iter_mut() {
                    if let LValue::Attr { recv, .. } = target {
                        self.analyze_expr(recv, ctx);
                    }
                    if let LValue::Index { recv, index } = target {
                        self.analyze_expr(recv, ctx);
                        self.analyze_expr(index, ctx);
                    }
                }
                value.ty.clone().unwrap_or_else(unknown)
            }

            ExprNode::While { cond, body, .. } => {
                self.analyze_expr(cond, ctx);
                self.analyze_expr(body, ctx);
                Ty::Nil
            }

            ExprNode::Range { begin, end, .. } => {
                let begin_ty = begin.as_mut().map(|b| self.analyze_expr(b, ctx));
                let end_ty = end.as_mut().map(|e| self.analyze_expr(e, ctx));
                // Type as Class { Range } parameterized by the
                // bound's element type. Both endpoints share a type
                // in well-formed Ruby (`1..10`, `"a".."z"`); when
                // either endpoint is missing (`1..` / `..10`), use
                // the available one. Falls back to Untyped if neither
                // endpoint is known — a beginless+endless range has
                // no element-type signal.
                let elem = begin_ty
                    .or(end_ty)
                    .unwrap_or(Ty::Untyped);
                Ty::Class {
                    id: ClassId(Symbol::from("Range")),
                    args: vec![elem],
                }
            }
            ExprNode::Cast { value, target_ty } => {
                // Visit the value so its inner sub-exprs get typed,
                // then return the explicit cast target — that's the
                // whole point of Cast: assert this expression has
                // `target_ty` regardless of what the value's flow
                // type computed to.
                let _ = self.analyze_expr(value, ctx);
                target_ty.clone()
            }
        }
    }

}

// Literal / primitive types ---------------------------------------------

pub(super) fn lit_ty(lit: &Literal) -> Ty {
    match lit {
        Literal::Nil => Ty::Nil,
        Literal::Bool { .. } => Ty::Bool,
        Literal::Int { .. } => Ty::Int,
        Literal::Float { .. } => Ty::Float,
        Literal::Str { .. } => Ty::Str,
        Literal::Sym { .. } => Ty::Sym,
        Literal::Regex { .. } => unknown(),
    }
}


pub(super) fn unknown() -> Ty {
    Ty::Var { var: TyVar(0) }
}

/// Stamp the expected type onto an empty Hash/Array literal so the
/// body-typer's normal inference (which would otherwise leave the
/// inner type as `Var`) carries the surrounding context. Used by
/// `Assign` to forward an ivar/var's declared type into a bare `{}`
/// or `[]` RHS — strict targets need the concrete shape (Crystal's
/// `{} of K => V`, TS's `Record<string, V>`) and `Var` doesn't
/// render. Non-empty literals are left alone; their inferred element
/// types already drive the result.
///
/// `expected` may arrive as `T | Nil` because pass-B reseeded ivars
/// union the assignment type with Nil to model first-read-before-
/// initialize; peel that wrapper before matching the container shape.
fn propagate_expected_to_empty_container(value: &mut Expr, expected: &Ty) {
    let peeled = peel_nilable(expected);
    match &mut *value.node {
        ExprNode::Hash { entries, .. } if entries.is_empty() => {
            if matches!(peeled, Ty::Hash { .. }) {
                value.ty = Some(peeled.clone());
            }
        }
        ExprNode::Array { elements, .. } if elements.is_empty() => {
            if matches!(peeled, Ty::Array { .. }) {
                value.ty = Some(peeled.clone());
            }
        }
        _ => {}
    }
}

fn peel_nilable(ty: &Ty) -> &Ty {
    if let Ty::Union { variants } = ty {
        if variants.len() == 2 {
            let nil_idx = variants.iter().position(|v| matches!(v, Ty::Nil));
            if let Some(idx) = nil_idx {
                return &variants[1 - idx];
            }
        }
    }
    ty
}

/// Type a single destructuring target of `a, b, ... = rhs` at
/// position `index`. Three RHS shapes carry a per-position type:
///   - `Array<T>` — every scalar target gets the element type `T`
///     (`@a, @b = list` where `list : Array<Story>`).
///   - `Tuple` — each target takes its positional element type.
///   - `Untyped` — the gradual escape flows to every target (the
///     pervasive `@x, @y = get_from_cache { ... }` /
///     `yield`-returning idiom, whose return type the analyzer can't
///     pin). Untyped reads surface as a gradual warning, not a hard
///     `ivar_unresolved` error.
/// Anything else yields `None` (no usable signal — leave the target
/// unbound rather than guess).
pub(crate) fn multiassign_target_ty(rhs: &Option<Ty>, index: usize) -> Option<Ty> {
    match rhs {
        Some(Ty::Array { elem }) => Some((**elem).clone()),
        Some(Ty::Tuple { elems }) => elems.get(index).cloned(),
        Some(Ty::Untyped) => Some(Ty::Untyped),
        _ => None,
    }
}

/// Walk an Expr collecting every `Assign { target: LValue::Var, .. }`
/// it contains, recording `name → expr.ty`. Used to thread local
/// bindings produced by an embedded assignment (`(x = find_by(...))`)
/// from a `BoolOp::And` left-arm or `If::cond` into the subsequent
/// arm's Ctx. The Seq-statement-level handler covers the common case
/// of top-level `x = ...` statements; this covers the nested form.
///
/// Descent stops at scope-introducing nodes (`Lambda`, `Let`) so a
/// block parameter assignment doesn't leak out.
fn collect_var_assignments_into(expr: &Expr, out: &mut HashMap<Symbol, Ty>) {
    match &*expr.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            if let Some(ty) = value.ty.clone() {
                out.insert(name.clone(), ty);
            }
            collect_var_assignments_into(value, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_var_assignments_into(left, out);
            collect_var_assignments_into(right, out);
        }
        ExprNode::Send { recv, args, .. } => {
            if let Some(r) = recv {
                collect_var_assignments_into(r, out);
            }
            for a in args {
                collect_var_assignments_into(a, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                collect_var_assignments_into(e, out);
            }
        }
        // Scope-introducing forms: Lambda's body and Let's binding
        // belong to inner scopes; their assignments don't escape.
        ExprNode::Lambda { .. } | ExprNode::Let { .. } => {}
        _ => {}
    }
}

pub(crate) fn union_of(a: Ty, b: Ty) -> Ty {
    // Bottom is the divergent-expression type — the branch carrying
    // it doesn't contribute a value, so it drops out of joins.
    // Mirrors Crystal's `Type.merge` filter on `NoReturnType`.
    // `if cond then raise else x end` types as `typeof(x)` instead
    // of `typeof(x) | Nil`.
    if matches!(a, Ty::Bottom) {
        return b;
    }
    if matches!(b, Ty::Bottom) {
        return a;
    }
    if a == b {
        return a;
    }
    // Structural join for same-shape generic containers — `Hash<A,B>
    // | Hash<C,D>` is more usefully expressed as `Hash<A|C, B|D>`
    // than as a flat Union, because dispatch on `.fetch` / `.[]` /
    // etc. operates on the Hash spine regardless of inner types.
    // The flat Union form would push every consumer to fan out per
    // variant. The if-narrowing case (`raw.is_a?(Hash) ? raw : {}`)
    // is the canonical producer — narrowed branch types `Hash<K,V>`,
    // empty-literal else branch types `Hash<Var,Var>`; the join
    // should be `Hash<K|Var, V|Var>`, which collapses to `Hash<K,V>`
    // once inference resolves the Vars.
    match (&a, &b) {
        (Ty::Hash { key: k1, value: v1 }, Ty::Hash { key: k2, value: v2 }) => {
            return Ty::Hash {
                key: Box::new(union_of((**k1).clone(), (**k2).clone())),
                value: Box::new(union_of((**v1).clone(), (**v2).clone())),
            };
        }
        (Ty::Array { elem: e1 }, Ty::Array { elem: e2 }) => {
            return Ty::Array {
                elem: Box::new(union_of((**e1).clone(), (**e2).clone())),
            };
        }
        _ => {}
    }
    // Flatten nested unions and drop duplicate variants. Without this,
    // joining `Int` into an existing `Int` (or `Union[Int]`) produced
    // `Int | Int` / `Union[Union[Int], Int]` — a degenerate union that
    // breaks binop classification (`Int > (Int | Int)`) and, when an
    // ivar like `@page` accumulates across many actions into the
    // layout-ivar union, nests dozens of levels deep.
    let mut variants = Vec::new();
    push_union_variants(a, &mut variants);
    push_union_variants(b, &mut variants);
    match variants.len() {
        0 => Ty::Bottom,
        1 => variants.into_iter().next().unwrap(),
        _ => Ty::Union { variants },
    }
}

/// Append `t`'s constituent variants to `out`, flattening nested
/// `Union`s and skipping any variant already present (structural
/// equality) so the result carries each distinct type once.
fn push_union_variants(t: Ty, out: &mut Vec<Ty>) {
    match t {
        Ty::Union { variants } => {
            for v in variants {
                push_union_variants(v, out);
            }
        }
        other => {
            if !out.contains(&other) {
                out.push(other);
            }
        }
    }
}

pub(super) fn union_many(tys: Vec<Ty>) -> Ty {
    // Fold through `union_of` so the same Bottom-filtering, structural
    // container join, and flatten/dedup normalization applies.
    let mut iter = tys.into_iter().filter(|t| !matches!(t, Ty::Bottom));
    match iter.next() {
        None => Ty::Bottom,
        Some(first) => iter.fold(first, union_of),
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use super::narrowing::{apply_narrowing, extract_narrowing};
    use crate::expr::ExprNode;
    use crate::ident::VarId;
    use crate::span::Span;

    fn synth(node: ExprNode) -> Expr {
        Expr::new(Span::synthetic(), node)
    }

    fn var(name: &str) -> Expr {
        synth(ExprNode::Var {
            id: VarId(0),
            name: Symbol::from(name),
        })
    }

    fn nil_lit() -> Expr {
        synth(ExprNode::Lit { value: Literal::Nil })
    }

    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        synth(ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        })
    }

    fn empty_classes() -> HashMap<ClassId, ClassInfo> {
        HashMap::new()
    }

    fn ctx_with_local(name: &str, ty: Ty) -> Ctx {
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(Symbol::from(name), ty);
        ctx
    }

    fn optional_str() -> Ty {
        Ty::Union {
            variants: vec![Ty::Str, Ty::Nil],
        }
    }

    #[test]
    fn if_nil_narrows_variable_to_nil_in_then_branch() {
        let cond = send(Some(var("x")), "nil?", vec![]);
        let pred = extract_narrowing(&cond).expect("narrowing detected");
        let ctx = ctx_with_local("x", optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Nil);
    }

    #[test]
    fn if_nil_narrows_variable_removing_nil_in_else_branch() {
        let cond = send(Some(var("x")), "nil?", vec![]);
        let pred = extract_narrowing(&cond).unwrap();
        let ctx = ctx_with_local("x", optional_str());
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(else_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
    }

    #[test]
    fn not_nil_is_inverse() {
        let inner = send(Some(var("x")), "nil?", vec![]);
        let cond = send(Some(inner), "!", vec![]);
        let pred = extract_narrowing(&cond).expect("negation recognized");
        let ctx = ctx_with_local("x", optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
        assert_eq!(else_ctx.local_bindings[&Symbol::from("x")], Ty::Nil);
    }

    #[test]
    fn explicit_equality_to_nil_narrows() {
        let cond = send(Some(var("x")), "==", vec![nil_lit()]);
        let pred = extract_narrowing(&cond).expect("== nil recognized");
        let ctx = ctx_with_local("x", optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Nil);
    }

    #[test]
    fn explicit_inequality_to_nil_narrows_inversely() {
        let cond = send(Some(var("x")), "!=", vec![nil_lit()]);
        let pred = extract_narrowing(&cond).expect("!= nil recognized");
        let ctx = ctx_with_local("x", optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
        assert_eq!(else_ctx.local_bindings[&Symbol::from("x")], Ty::Nil);
    }

    #[test]
    fn ivar_narrowing() {
        let ivar = synth(ExprNode::Ivar {
            name: Symbol::from("post"),
        });
        let cond = send(Some(ivar), "nil?", vec![]);
        let pred = extract_narrowing(&cond).unwrap();
        let mut ctx = Ctx::default();
        ctx.ivar_bindings
            .insert(Symbol::from("post"), optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.ivar_bindings[&Symbol::from("post")], Ty::Nil);
        assert_eq!(else_ctx.ivar_bindings[&Symbol::from("post")], Ty::Str);
    }

    #[test]
    fn missing_binding_is_a_noop() {
        let cond = send(Some(var("x")), "nil?", vec![]);
        let pred = extract_narrowing(&cond).unwrap();
        let ctx = Ctx::default();
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert!(then_ctx.local_bindings.is_empty());
    }

    #[test]
    fn non_narrowing_condition_returns_none() {
        let cond = send(Some(var("x")), "length", vec![]);
        assert!(extract_narrowing(&cond).is_none());
    }

    #[test]
    fn is_a_string_narrows_to_str() {
        let class_ref = synth(ExprNode::Const {
            path: vec![Symbol::from("String")],
        });
        let cond = send(Some(var("x")), "is_a?", vec![class_ref]);
        let pred = extract_narrowing(&cond).expect("is_a? recognized");
        let mixed = Ty::Union {
            variants: vec![Ty::Str, Ty::Int, Ty::Nil],
        };
        let ctx = ctx_with_local("x", mixed);
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
        assert_eq!(
            else_ctx.local_bindings[&Symbol::from("x")],
            Ty::Union {
                variants: vec![Ty::Int, Ty::Nil],
            }
        );
    }

    #[test]
    fn is_a_user_class_narrows_to_class_ty() {
        let class_ref = synth(ExprNode::Const {
            path: vec![Symbol::from("Post")],
        });
        let cond = send(Some(var("x")), "is_a?", vec![class_ref]);
        let pred = extract_narrowing(&cond).unwrap();
        let ctx = ctx_with_local(
            "x",
            Ty::Class {
                id: ClassId(Symbol::from("Post")),
                args: vec![],
            },
        );
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert_eq!(
            then_ctx.local_bindings[&Symbol::from("x")],
            Ty::Class {
                id: ClassId(Symbol::from("Post")),
                args: vec![]
            }
        );
    }

    #[test]
    fn end_to_end_if_nil_narrows_through_analyzer() {
        // Build: `if x.nil?; x; else; x.length; end` — with x: String | Nil,
        // the If's type should be Nil | Int (then is Nil, else is Int
        // because x narrows to String and String#length → Int).
        let then_branch = var("x");
        let else_branch = send(Some(var("x")), "length", vec![]);
        let if_expr = synth(ExprNode::If {
            cond: send(Some(var("x")), "nil?", vec![]),
            then_branch,
            else_branch,
        });
        let mut expr = if_expr;

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = ctx_with_local("x", optional_str());
        let t = typer.analyze_expr(&mut expr, &ctx);

        // Result should be the union of (Nil) | (Int) — i.e., { Nil, Int }.
        let variants = match t {
            Ty::Union { variants } => variants,
            other => panic!("expected Union, got {other:?}"),
        };
        assert!(variants.contains(&Ty::Nil), "variants: {variants:?}");
        assert!(variants.contains(&Ty::Int), "variants: {variants:?}");
    }

    #[test]
    fn narrowing_writes_back_to_var_ty_inside_assign_in_else_branch() {
        // `if x.nil? then nil else @y = x end` — same shape as the
        // postfix `@y = x unless x.nil?` form. The Var{x} sits inside
        // an Assign, which is inside the else-branch. The Var's .ty
        // should still narrow to String — i.e. write-back has to
        // propagate through every nested node the body-typer walks.
        let then_branch = nil_lit();
        let assign = synth(ExprNode::Assign {
            target: LValue::Ivar { name: Symbol::from("y") },
            value: var("x"),
        });
        let if_expr = synth(ExprNode::If {
            cond: send(Some(var("x")), "nil?", vec![]),
            then_branch,
            else_branch: assign,
        });
        let mut expr = if_expr;

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = ctx_with_local("x", optional_str());
        typer.analyze_expr(&mut expr, &ctx);

        let ExprNode::If { else_branch, .. } = &*expr.node else {
            panic!("expected If");
        };
        let ExprNode::Assign { value, .. } = &*else_branch.node else {
            panic!("expected Assign in else");
        };
        assert_eq!(
            value.ty,
            Some(Ty::Str),
            "Var{{x}} inside Assign{{value:}} in else should narrow to Str; \
             got {:?}",
            value.ty,
        );
    }

    #[test]
    fn narrowing_writes_back_to_var_ty_in_else_branch() {
        // `if x.nil? then nil else x end` — x narrows to String in the
        // else branch; the Var{x} read inside else should carry ty=Str
        // on its Expr, not the un-narrowed Option<String>. This is the
        // "narrowing write-back" gate — emit-side coercion paths read
        // value.ty directly, and a stale Option<String> there triggers
        // spurious .to_string().unwrap() chains (E0599 on rust2's
        // action_controller_base render).
        let then_branch = nil_lit();
        let else_branch = var("x");
        let if_expr = synth(ExprNode::If {
            cond: send(Some(var("x")), "nil?", vec![]),
            then_branch,
            else_branch,
        });
        let mut expr = if_expr;

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = ctx_with_local("x", optional_str());
        typer.analyze_expr(&mut expr, &ctx);

        let ExprNode::If { else_branch, .. } = &*expr.node else {
            panic!("expected If");
        };
        assert_eq!(
            else_branch.ty,
            Some(Ty::Str),
            "else-branch Var{{x}} should reflect narrowed (non-nil) type"
        );
    }

    // ── block return propagation (7b) ──────────────────────────────

    use crate::expr::BlockStyle;

    fn lambda(params: Vec<&str>, body: Expr) -> Expr {
        synth(ExprNode::Lambda {
            params: params.into_iter().map(Symbol::from).collect(),
            block_param: None,
            body,
            block_style: BlockStyle::Do,
        })
    }

    #[test]
    fn array_map_returns_block_body_type() {
        // arr.map { |x| x.to_s } on arr: Array[Int] should produce Array[Str]
        let arr = {
            let mut e = var("arr");
            e.ty = Some(Ty::Array { elem: Box::new(Ty::Int) });
            e
        };
        let block_body = send(Some(var("x")), "to_s", vec![]);
        let lam = lambda(vec!["x"], block_body);
        let call = synth(ExprNode::Send {
            recv: Some(arr),
            method: Symbol::from("map"),
            args: vec![],
            block: Some(lam),
            parenthesized: false,
        });

        let mut expr = call;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(
            Symbol::from("arr"),
            Ty::Array { elem: Box::new(Ty::Int) },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: Box::new(Ty::Str) });
    }

    #[test]
    fn array_select_preserves_element_type() {
        // arr.select { |x| x > 0 } on arr: Array[Int] should still be Array[Int]
        let arr = {
            let mut e = var("arr");
            e.ty = Some(Ty::Array { elem: Box::new(Ty::Int) });
            e
        };
        let block_body = send(Some(var("x")), ">", vec![synth(ExprNode::Lit {
            value: Literal::Int { value: 0 },
        })]);
        let lam = lambda(vec!["x"], block_body);
        let call = synth(ExprNode::Send {
            recv: Some(arr),
            method: Symbol::from("select"),
            args: vec![],
            block: Some(lam),
            parenthesized: false,
        });

        let mut expr = call;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(
            Symbol::from("arr"),
            Ty::Array { elem: Box::new(Ty::Int) },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: Box::new(Ty::Int) });
    }

    #[test]
    fn array_flat_map_flattens_one_level() {
        // arr.flat_map { |x| [x.to_s] } on arr: Array[Int] should be Array[Str]
        let arr = {
            let mut e = var("arr");
            e.ty = Some(Ty::Array { elem: Box::new(Ty::Int) });
            e
        };
        let inner_arr = synth(ExprNode::Array {
            elements: vec![send(Some(var("x")), "to_s", vec![])],
            style: Default::default(),
        });
        let lam = lambda(vec!["x"], inner_arr);
        let call = synth(ExprNode::Send {
            recv: Some(arr),
            method: Symbol::from("flat_map"),
            args: vec![],
            block: Some(lam),
            parenthesized: false,
        });

        let mut expr = call;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(
            Symbol::from("arr"),
            Ty::Array { elem: Box::new(Ty::Int) },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: Box::new(Ty::Str) });
    }

    #[test]
    fn hash_map_returns_array_of_block_ret() {
        // h.map { |k, v| v.to_s } on h: Hash[Sym, Int] should be Array[Str]
        let h = {
            let mut e = var("h");
            e.ty = Some(Ty::Hash {
                key: Box::new(Ty::Sym),
                value: Box::new(Ty::Int),
            });
            e
        };
        let block_body = send(Some(var("v")), "to_s", vec![]);
        let lam = lambda(vec!["k", "v"], block_body);
        let call = synth(ExprNode::Send {
            recv: Some(h),
            method: Symbol::from("map"),
            args: vec![],
            block: Some(lam),
            parenthesized: false,
        });

        let mut expr = call;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(
            Symbol::from("h"),
            Ty::Hash {
                key: Box::new(Ty::Sym),
                value: Box::new(Ty::Int),
            },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: Box::new(Ty::Str) });
    }

    #[test]
    fn hash_transform_values_changes_value_type() {
        // h.transform_values { |v| v.to_s } on Hash[Sym, Int] → Hash[Sym, Str]
        let h = {
            let mut e = var("h");
            e.ty = Some(Ty::Hash {
                key: Box::new(Ty::Sym),
                value: Box::new(Ty::Int),
            });
            e
        };
        let block_body = send(Some(var("v")), "to_s", vec![]);
        let lam = lambda(vec!["v"], block_body);
        let call = synth(ExprNode::Send {
            recv: Some(h),
            method: Symbol::from("transform_values"),
            args: vec![],
            block: Some(lam),
            parenthesized: false,
        });

        let mut expr = call;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(
            Symbol::from("h"),
            Ty::Hash {
                key: Box::new(Ty::Sym),
                value: Box::new(Ty::Int),
            },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(
            ty,
            Ty::Hash {
                key: Box::new(Ty::Sym),
                value: Box::new(Ty::Str),
            }
        );
    }

    #[test]
    fn map_without_block_falls_back_to_input_elem() {
        // arr.map (no block — Symbol-to-Proc pattern handled elsewhere)
        // should still produce a sensible Array[elem] type.
        let arr = {
            let mut e = var("arr");
            e.ty = Some(Ty::Array { elem: Box::new(Ty::Int) });
            e
        };
        let call = synth(ExprNode::Send {
            recv: Some(arr),
            method: Symbol::from("map"),
            args: vec![],
            block: None,
            parenthesized: false,
        });

        let mut expr = call;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(
            Symbol::from("arr"),
            Ty::Array { elem: Box::new(Ty::Int) },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: Box::new(Ty::Int) });
    }

    // ── diagnostic annotation ─────────────────────────────────────

    #[test]
    fn incompatible_add_annotates_diagnostic_on_send() {
        // Build: `1 + "hello"` — concrete Int + Str. Body-typer should
        // annotate the enclosing Send with an IncompatibleBinop
        // diagnostic.
        let lhs = synth(ExprNode::Lit { value: Literal::Int { value: 1 } });
        let rhs = synth(ExprNode::Lit {
            value: Literal::Str { value: "hello".to_string() },
        });
        let add = synth(ExprNode::Send {
            recv: Some(lhs),
            method: Symbol::from("+"),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        });

        let mut expr = add;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        typer.analyze_expr(&mut expr, &Ctx::default());

        let diag = expr.diagnostic.as_ref().expect("diagnostic set");
        match diag {
            crate::diagnostic::DiagnosticKind::IncompatibleBinop {
                op,
                lhs_ty,
                rhs_ty,
            } => {
                assert_eq!(op.as_str(), "+");
                assert_eq!(lhs_ty, &Ty::Int);
                assert_eq!(rhs_ty, &Ty::Str);
            }
            other => panic!("expected IncompatibleBinop, got {other:?}"),
        }
    }

    #[test]
    fn incompatible_compare_annotates_diagnostic_on_send() {
        // `1 < "hello"` — Ruby's Comparable raises on mixed types.
        // Body-typer should annotate the Send.
        let lhs = synth(ExprNode::Lit { value: Literal::Int { value: 1 } });
        let rhs = synth(ExprNode::Lit {
            value: Literal::Str { value: "hello".to_string() },
        });
        let cmp = synth(ExprNode::Send {
            recv: Some(lhs),
            method: Symbol::from("<"),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        });

        let mut expr = cmp;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        typer.analyze_expr(&mut expr, &Ctx::default());

        let diag = expr.diagnostic.as_ref().expect("diagnostic set");
        match diag {
            crate::diagnostic::DiagnosticKind::IncompatibleBinop {
                op,
                lhs_ty,
                rhs_ty,
            } => {
                assert_eq!(op.as_str(), "<");
                assert_eq!(lhs_ty, &Ty::Int);
                assert_eq!(rhs_ty, &Ty::Str);
            }
            other => panic!("expected IncompatibleBinop, got {other:?}"),
        }
    }

    #[test]
    fn compatible_compare_leaves_diagnostic_empty() {
        // Int < Int is valid.
        let lhs = synth(ExprNode::Lit { value: Literal::Int { value: 1 } });
        let rhs = synth(ExprNode::Lit { value: Literal::Int { value: 2 } });
        let cmp = synth(ExprNode::Send {
            recv: Some(lhs),
            method: Symbol::from("<"),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        });

        let mut expr = cmp;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        typer.analyze_expr(&mut expr, &Ctx::default());

        assert!(expr.diagnostic.is_none());
    }

    #[test]
    fn incompatible_sub_annotates_diagnostic_on_send() {
        // `"a" - "b"` — Ruby's String doesn't define `-`, NoMethodError.
        let lhs = synth(ExprNode::Lit { value: Literal::Str { value: "a".to_string() } });
        let rhs = synth(ExprNode::Lit { value: Literal::Str { value: "b".to_string() } });
        let sub = synth(ExprNode::Send {
            recv: Some(lhs),
            method: Symbol::from("-"),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        });

        let mut expr = sub;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        typer.analyze_expr(&mut expr, &Ctx::default());

        let diag = expr.diagnostic.as_ref().expect("diagnostic set");
        match diag {
            crate::diagnostic::DiagnosticKind::IncompatibleBinop {
                op, lhs_ty, rhs_ty,
            } => {
                assert_eq!(op.as_str(), "-");
                assert_eq!(lhs_ty, &Ty::Str);
                assert_eq!(rhs_ty, &Ty::Str);
            }
            other => panic!("expected IncompatibleBinop, got {other:?}"),
        }
    }

    #[test]
    fn compatible_sub_leaves_diagnostic_empty() {
        // Int - Int is valid.
        let lhs = synth(ExprNode::Lit { value: Literal::Int { value: 5 } });
        let rhs = synth(ExprNode::Lit { value: Literal::Int { value: 2 } });
        let sub = synth(ExprNode::Send {
            recv: Some(lhs),
            method: Symbol::from("-"),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        });

        let mut expr = sub;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        typer.analyze_expr(&mut expr, &Ctx::default());

        assert!(expr.diagnostic.is_none());
    }

    #[test]
    fn incompatible_mul_annotates_diagnostic_on_send() {
        // `{} * {}` — Hash * Hash is NoMethodError in Ruby.
        let h = || {
            let mut e = synth(ExprNode::Hash {
                entries: vec![],
                kwargs: false,
            });
            e.ty = Some(Ty::Hash {
                key: Box::new(Ty::Sym),
                value: Box::new(Ty::Int),
            });
            e
        };
        let mul = synth(ExprNode::Send {
            recv: Some(h()),
            method: Symbol::from("*"),
            args: vec![h()],
            block: None,
            parenthesized: false,
        });

        let mut expr = mul;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        typer.analyze_expr(&mut expr, &Ctx::default());

        let diag = expr.diagnostic.as_ref().expect("diagnostic set");
        assert!(matches!(
            diag,
            crate::diagnostic::DiagnosticKind::IncompatibleBinop { op, .. } if op.as_str() == "*"
        ));
    }

    #[test]
    fn incompatible_div_annotates_diagnostic_on_send() {
        // `"a" / 2` — String doesn't define `/`.
        let lhs = synth(ExprNode::Lit { value: Literal::Str { value: "a".to_string() } });
        let rhs = synth(ExprNode::Lit { value: Literal::Int { value: 2 } });
        let div = synth(ExprNode::Send {
            recv: Some(lhs),
            method: Symbol::from("/"),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        });

        let mut expr = div;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        typer.analyze_expr(&mut expr, &Ctx::default());

        let diag = expr.diagnostic.as_ref().expect("diagnostic set");
        assert!(matches!(
            diag,
            crate::diagnostic::DiagnosticKind::IncompatibleBinop { op, .. } if op.as_str() == "/"
        ));
    }

    #[test]
    fn compatible_add_leaves_diagnostic_empty() {
        // Int + Int must NOT be annotated — it's valid Ruby.
        let lhs = synth(ExprNode::Lit { value: Literal::Int { value: 1 } });
        let rhs = synth(ExprNode::Lit { value: Literal::Int { value: 2 } });
        let add = synth(ExprNode::Send {
            recv: Some(lhs),
            method: Symbol::from("+"),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        });

        let mut expr = add;
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        typer.analyze_expr(&mut expr, &Ctx::default());

        assert!(expr.diagnostic.is_none());
    }
}
