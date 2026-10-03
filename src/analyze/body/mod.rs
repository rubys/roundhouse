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
use rubydex::model::identity_maps::IdentityHashMap;
use rubydex::model::ids::{DeclarationId, declaration_id_from_lookup_name};

use crate::expr::{Expr, ExprNode, HashRest, LValue, Literal, MatchPattern};
use crate::ident::{ClassId, Symbol, TyVar};
use crate::ty::{Row, Ty};

mod diagnostic;
mod const_resolution;
pub(crate) use const_resolution::{ConstResolver, ConstResolverTask};
pub use const_resolution::PreparedConstResolver;
use const_resolution::ResolvedConstant;
mod narrowing;
mod send;
pub(crate) use send::PARAM_VALUE;
pub(crate) use send::string_answers;

/// Typed constants for generated expressions without Ruby source spans.
/// The class's constants shadow the shared app registry. Source-backed
/// expressions use Rubydex declaration IDs instead of this scope.
#[derive(Clone, Default)]
pub struct ConstScope {
    own: std::sync::Arc<HashMap<Symbol, Ty>>,
    global: std::sync::Arc<HashMap<Symbol, Ty>>,
}

impl ConstScope {
    /// Just the app-wide registry.
    pub fn global(map: HashMap<Symbol, Ty>) -> Self {
        Self { own: Default::default(), global: std::sync::Arc::new(map) }
    }

    /// This scope's registry with `own` layered over it (a later entry
    /// wins, as `extend` would).
    pub fn with_own(&self, own: impl IntoIterator<Item = (Symbol, Ty)>) -> Self {
        Self { own: std::sync::Arc::new(own.into_iter().collect()), global: self.global.clone() }
    }

    pub fn get(&self, name: &Symbol) -> Option<&Ty> {
        self.own.get(name).or_else(|| self.global.get(name))
    }
}

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
    /// Module/class-level typed constants such as
    /// `STATUS_CODES = { ok: 200, ... }.freeze`. Rubydex IDs resolve
    /// source-backed reads; this scope types generated expressions.
    ///
    /// SHARED, not owned: written once when a context is built and only
    /// read after, while `Ctx` is cloned for every block, branch and
    /// method the typer enters. As an owned map each of those clones
    /// copied the whole registry — every constant in the app, merged
    /// into each class's view of it — and on Mastodon that copying was
    /// most of the analysis time.
    pub constants: ConstScope,
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
    /// Which of `class_methods` had their return type inferred FROM a
    /// query chain over this model — every `scope`, plus any class
    /// method whose body tail is such a chain.
    ///
    /// Relation- and Array-receiver dispatch delegates to the element
    /// model's class-method table, and this is the membership test that
    /// keeps the delegation honest. Forwarding arbitrary class methods
    /// would be Rails' method_missing semantics; an unresolved send
    /// should surface rather than silently forward. But a method whose
    /// body IS a relation chain over this model is exactly what Rails
    /// delegates through `scoping`, and it is the one case where the
    /// answer is knowable rather than guessed.
    ///
    /// The relation-RETURNING half of that surface already delegated by
    /// matching on `Ty::Relation`. This set extends it to the half that
    /// TERMINATES — campfire's `def self.original; order(:created_at)
    /// .first; end`, reached as `Current.user.rooms.original`, whose
    /// return is a record and so matches no relation shape. Without it
    /// the chain typed to nothing and `room_url(…)` rendered
    /// `/rooms/#<Room:0x…>`.
    pub relation_derived: std::collections::HashSet<Symbol>,
    /// Scopes whose body the seed classifier read as ending in a
    /// MATERIALIZING terminal — `ordered.last(PAGE_SIZE)` — as opposed
    /// to the ones it could not read at all. Both kinds may carry the
    /// `Array[Self]` seed (the counted terminal by classification, the
    /// unreadable body as the conservative stand-in), and only this set
    /// tells them apart at a Relation receiver: a materializing scope
    /// answers its Array, an unclassified one is assumed to still be a
    /// query and re-wraps as the relation.
    pub materializing_scopes: std::collections::HashSet<Symbol>,
    /// Methods whose every return is the value of the BLOCK they were
    /// called with — `yield` on each path, or the block handed on to
    /// another such method (lobsters' `get_from_cache(opts, &)`, which
    /// answers `yield` uncached and `Rails.cache.fetch(key, &)` cached).
    /// No signature can say that without method generics, which the RBS
    /// reader does not model, so dispatch answers the CALL SITE's block
    /// type for these, the way it already does for `then` and
    /// `transaction`. Harvested each fixpoint round
    /// (`Analyzer::harvest_method_returns`).
    pub block_value_methods: std::collections::HashSet<Symbol>,
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
    /// `has_many :memberships do def grant_to(users) … end end` — the
    /// methods an association EXTENSION BLOCK declares, keyed
    /// `(association, method)`.
    ///
    /// They cannot live in `instance_methods` of either class
    /// involved: they are not the owner's (you call them on the
    /// association) and not the target's (`Membership` has no
    /// `grant_to`). And the association read types `Array<Membership>`,
    /// which has forgotten which association produced it — so the
    /// dispatch arms cannot find them from the receiver's TYPE at all.
    /// The Send arm looks them up from the receiver's SHAPE instead,
    /// which is the same two hops the emit-time flattening reads
    /// (`room.memberships.grant_to(u)` → `room.memberships_grant_to(u)`).
    pub assoc_extensions: HashMap<(Symbol, Symbol), Ty>,
    /// Modules mixed in via `include` (e.g. a controller's
    /// `include IntervalHelper`). A mixed-in module's instance methods
    /// become instance methods of the includer, so dispatch consults
    /// these (their own instance_methods in the registry) after the
    /// class's own methods and before walking the parent. Empty for
    /// most classes.
    pub includes: Vec<crate::ident::ClassId>,
}


/// Reusable body-type walker. Holds a borrow of the dispatch table so
/// repeated `analyze_expr` calls reuse the same lookup structures
/// without cloning.
pub struct BodyTyper<'a> {
    classes: &'a HashMap<ClassId, ClassInfo>,
    const_resolver: Option<std::sync::Arc<ConstResolver>>,
    typed_constants: Option<&'a IdentityHashMap<DeclarationId, Ty>>,
    data_factories: Option<&'a HashMap<crate::span::Span, Ty>>,
    /// Methods whose value is an ActiveSupport inquirer (see
    /// [`crate::analyze::inquiry`]); empty for the bare constructor,
    /// which the runtime-source typer and tests use.
    inquirers: Option<&'a std::collections::HashSet<Symbol>>,
}

impl<'a> BodyTyper<'a> {
    /// Submodule accessor for the dispatch table. `classes` itself
    /// stays private to this module; `send.rs` reaches it here.
    pub(super) fn classes(&self) -> &'a HashMap<ClassId, ClassInfo> {
        self.classes
    }

    pub fn new(classes: &'a HashMap<ClassId, ClassInfo>) -> Self {
        Self { classes, const_resolver: None, typed_constants: None, data_factories: None, inquirers: None }
    }

    /// Share the analyzer's immutable source index across typing passes.
    pub(crate) fn with_const_resolver(mut self, resolver: std::sync::Arc<ConstResolver>) -> Self {
        self.const_resolver = Some(resolver);
        self
    }

    /// Roundhouse infers values; Rubydex supplies their declaration IDs.
    pub(super) fn with_typed_constants(
        mut self,
        values: &'a IdentityHashMap<DeclarationId, Ty>,
    ) -> Self {
        self.typed_constants = Some(values);
        self
    }

    pub(super) fn with_data_factories(mut self, factories: &'a HashMap<crate::span::Span, Ty>) -> Self {
        self.data_factories = Some(factories);
        self
    }

    /// Name the app's inquirer-returning methods, so their predicates
    /// type as Bool instead of failing dispatch on `Str`.
    pub fn with_inquirers(
        mut self,
        inquirers: &'a std::collections::HashSet<Symbol>,
    ) -> Self {
        self.inquirers = Some(inquirers);
        self
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

    /// Pattern-side expressions use ordinary expression/constant resolution;
    /// recognizing pattern syntax does not grant unknown classes new support.
    fn analyze_match_pattern_constants(&self, pattern: &mut MatchPattern, ctx: &Ctx) {
        pattern.for_each_expr_mut(&mut |expr| { self.analyze_expr(expr, ctx); });
    }

    /// The locals a `MatchPattern` binds when it matches, typed against
    /// `subject_ty` — the static type of the value being tested AT
    /// THIS POSITION (the `CaseMatch` scrutinee at the top level, `None`
    /// for anything nested inside an `Array`/`Find` pattern, whose
    /// `deconstruct` return isn't modeled positionally).
    ///
    /// Only a *plain* `Hash` pattern (no `constant` narrowing) against a
    /// statically `Hash[Symbol, V]`-typed subject gets typed key
    /// bindings, and every key gets the SAME `V` — `Ty::Hash` has no
    /// per-key shape, so `{status:, data:}` against `Hash[Sym, String]`
    /// types both `status` and `data` as `String`, which is exactly
    /// right for a value-omission bind (`data:`) and merely
    /// conservative for one with an explicit sub-pattern. A
    /// `constant`-narrowed `Hash`/`Array` pattern (`Success(value:)`,
    /// `Success(page)`) always types its bindings `Untyped`: reading a
    /// narrowing class's own attribute types back out (real
    /// `deconstruct_keys` fidelity) is future work, and every one of
    /// those constants in the Procore survey is a gem class the
    /// registry doesn't know regardless, so today the two cases collapse
    /// to the same answer.
    fn match_pattern_bindings(
        &self,
        pattern: &MatchPattern,
        subject_ty: Option<&Ty>,
    ) -> Vec<(Symbol, Ty)> {
        match pattern {
            MatchPattern::Value { .. } | MatchPattern::Nil => Vec::new(),
            MatchPattern::Bind { name } => {
                vec![(name.clone(), subject_ty.cloned().unwrap_or(Ty::Untyped))]
            }
            MatchPattern::Capture { pattern, name } => {
                let mut out = self.match_pattern_bindings(pattern, subject_ty);
                out.push((name.clone(), subject_ty.cloned().unwrap_or(Ty::Untyped)));
                out
            }
            // Ruby permits `_`-prefixed bindings in alternatives. A
            // different alternative can succeed without that binding.
            MatchPattern::Alt { alternatives } => alternatives.iter()
                .flat_map(|p| self.match_pattern_bindings(p, subject_ty))
                .map(|(name, ty)| (name, union_of(ty, Ty::Nil))).collect(),
            MatchPattern::Array { pre, rest, post, .. } => {
                let mut out = Vec::new();
                for p in pre.iter().chain(post.iter()) {
                    out.extend(self.match_pattern_bindings(p, None));
                }
                if let Some(Some(name)) = rest {
                    out.push((name.clone(), Ty::Untyped));
                }
                out
            }
            MatchPattern::Find { middle, pre_rest, post_rest, .. } => {
                let mut out = Vec::new();
                for p in middle {
                    out.extend(self.match_pattern_bindings(p, None));
                }
                if let Some(name) = pre_rest {
                    out.push((name.clone(), Ty::Untyped));
                }
                if let Some(name) = post_rest {
                    out.push((name.clone(), Ty::Untyped));
                }
                out
            }
            MatchPattern::Hash { constant, pairs, rest } => {
                let value_ty = if constant.is_none() {
                    match subject_ty {
                        Some(Ty::Hash { key, value }) if matches!(**key, Ty::Sym) => {
                            Some((**value).clone())
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                let mut out = Vec::new();
                for (key, sub) in pairs {
                    match sub {
                        Some(p) => out.extend(self.match_pattern_bindings(p, value_ty.as_ref())),
                        None => out.push((key.clone(), value_ty.clone().unwrap_or(Ty::Untyped))),
                    }
                }
                if let Some(HashRest::Collect { name }) = rest {
                    // The rest is a Hash returned by deconstruct_keys,
                    // never the deconstructed object. Unmatched keys
                    // need not be symbols, even though pattern keys are.
                    let rest_ty = match subject_ty {
                        Some(ty @ Ty::Hash { .. }) if constant.is_none() => ty.clone(),
                        _ => Ty::Hash { key: Box::new(Ty::Untyped), value: Box::new(Ty::Untyped) },
                    };
                    out.push((name.clone(), rest_ty));
                }
                out
            }
        }
    }

    /// Matches nested in expressions can bind locals too. Conditional
    /// matches may keep a prior/nil value; a required-match statement
    /// definitely binds on the continuing path.
    fn propagate_match_bindings(&self, expr: &Expr, ctx: &mut Ctx, definite: bool) {
        if matches!(&*expr.node, ExprNode::Lambda { .. } | ExprNode::Let { .. }) { return; }
        let mut bind = |pattern: &MatchPattern, subject_ty: Option<&Ty>| {
            for (name, ty) in self.match_pattern_bindings(pattern, subject_ty) {
                let ty = if definite && matches!(&*expr.node, ExprNode::MatchRequired { .. }) {
                    ty
                } else {
                    union_of(ctx.local_bindings.get(&name).cloned().unwrap_or(Ty::Nil), ty)
                };
                ctx.local_bindings.insert(name, ty);
            }
        };
        match &*expr.node {
            ExprNode::CaseMatch { scrutinee, arms, .. } => {
                for arm in arms { bind(&arm.pattern, scrutinee.ty.as_ref()); }
            }
            ExprNode::MatchPredicate { value, pattern } | ExprNode::MatchRequired { value, pattern } => {
                bind(pattern, value.ty.as_ref());
            }
            _ => {}
        }
        expr.node.for_each_child(&mut |child| self.propagate_match_bindings(child, ctx, false));
    }

    fn compute(&self, expr: &mut Expr, ctx: &Ctx) -> Ty {
        let expr_span = expr.span;
        match &mut *expr.node {
            ExprNode::Lit { value } => lit_ty(value),

            ExprNode::Const { path } => {
                // A later typing pass may resolve a value constant whose
                // owner was unknown in an earlier constant fixpoint round.
                expr.diagnostic = None;
                // Never search by suffix or borrow another scope's
                // same-named declaration: only the exact written class.
                let exact_modeled_class = |path: &[Symbol]| {
                    let id = written_class_id(path);
                    if self.classes().contains_key(&id) {
                        Ty::Class { id, args: vec![] }
                    } else {
                        unknown()
                    }
                };
                let indexed_source = !expr_span.is_synthetic()
                    && self
                        .const_resolver
                        .as_ref()
                        .is_some_and(|resolver| resolver.has_source_file(expr_span.file));
                let source_reference = self
                    .const_resolver
                    .as_ref()
                    .and_then(|resolver| resolver.reference(expr_span, path));
                let ty = match source_reference {
                    Some(Some(ResolvedConstant::Namespace { class, runtime })) => {
                        let id = class.as_ref().clone();
                        if self.classes().contains_key(&id) || *runtime {
                            expr.decisions |= crate::expr::RESOLVED_CLASS_REF;
                            qualify_resolved_path(path, &id);
                            Ty::Class { id, args: vec![] }
                        } else {
                            unknown()
                        }
                    }
                    Some(Some(ResolvedConstant::Value { declaration, runtime })) => self.typed_constants
                        .and_then(|values| values.get(declaration))
                        .cloned()
                        .or_else(|| runtime.as_ref().map(|ty| (**ty).clone()))
                        .unwrap_or_else(unknown),
                    // An unresolved source reference may still name an
                    // exact modeled external class (for example Time).
                    Some(None) => exact_modeled_class(path),
                    // No reference in an indexed file: a lowering pass
                    // generated this node with a borrowed source span
                    // (Alba's serializers).
                    None if indexed_source => exact_modeled_class(path),
                    // Views and generated IR have no Rubydex answer. A
                    // bare name can use the app's bare-name values. A
                    // qualified name `A::B` names its owner, so only the
                    // value declared at that full name answers it.
                    None => {
                        let value = if let [name] = path.as_slice() {
                            ctx.constants.get(name).cloned()
                        } else {
                            let name = written_class_id(path);
                            self.typed_constants
                                .and_then(|values| {
                                    values.get(&declaration_id_from_lookup_name(name.0.as_str()))
                                })
                                .cloned()
                        };
                        // Otherwise retain the written class path, but
                        // never guess another class by its suffix.
                        value.unwrap_or_else(|| Ty::Class { id: written_class_id(path), args: vec![] })
                    }
                };
                if indexed_source && matches!(ty, Ty::Var { .. }) {
                    expr.diagnostic = Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("constant"),
                        detail: written_class_id(path).0.as_str().to_string(),
                    });
                }
                ty
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
                    if let Some(name) = &rc.binding {
                        // An unregistered class (a gem's error) stays StandardError: typed as itself, even `e.message` would fail.
                        let rescued = match rc.classes.as_slice() {
                            [c] => match &c.ty {
                                Some(ty @ Ty::Class { id, .. }) if self.classes().contains_key(id) => Some(ty.clone()),
                                _ => None,
                            },
                            _ => None,
                        };
                        let mut inner = ctx.clone();
                        inner.local_bindings.insert(
                            name.clone(),
                            rescued.unwrap_or_else(|| Ty::Class {
                                id: crate::ident::ClassId(Symbol::from("StandardError")),
                                args: vec![],
                            }),
                        );
                        self.analyze_expr(&mut rc.body, &inner);
                        continue;
                    }
                    self.analyze_expr(&mut rc.body, ctx);
                }
                if let Some(e) = else_branch {
                    self.analyze_expr(e, ctx);
                }
                if let Some(e) = ensure {
                    self.analyze_expr(e, ctx);
                }
                // Ruby's value here is the ELSE branch when there is
                // one (it runs on the no-exception path and its value
                // wins over the begin body's), otherwise the begin
                // body's — unioned with every rescue body, which is the
                // value on the handled path. `ensure` never contributes
                // a value.
                //
                // Dropping the rescue arms — what this did before —
                // makes `uri.to_s … rescue nil` infer `String` where it
                // answers `String?`, and the emitted RBS then
                // OVER-CLAIMS. campfire's
                // `Metadata.replace_twitter_domain_for_opengraph_support`
                // is exactly that shape: signed `-> String`, returning
                // nil from `rescue URI::InvalidURIError`, and the C
                // build stopped on `returning 'sp_RbVal' from a
                // function with incompatible result type 'const char *'`.
                //
                // `Bottom` arms are SKIPPED rather than unioned: a
                // `rescue` that only re-raises diverges and must not
                // drag the whole expression to Bottom. Same rule
                // `effective_return_ty` applies to a diverging tail.
                let mut arms: Vec<Ty> = Vec::new();
                let primary = else_branch
                    .as_ref()
                    .and_then(|e| e.ty.clone())
                    .or_else(|| body.ty.clone());
                if let Some(t) = primary {
                    if !matches!(t, Ty::Bottom) {
                        arms.push(t);
                    }
                }
                for rc in rescues.iter() {
                    if let Some(t) = &rc.body.ty {
                        if !matches!(t, Ty::Bottom) {
                            arms.push(t.clone());
                        }
                    }
                }
                if arms.is_empty() { unknown() } else { union_many(arms) }
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
                // Var assignments inside `left` (the canonical case is
                // `(x = find_by(...)) && x.foo`) need to flow into
                // `right`'s scope. Without this, `x` reads as Var
                // because the Seq-level Assign-to-Var handler only
                // catches statement-position assignments, not nested
                // ones inside expressions. For `&&` the assignment
                // executed (left was truthy); for `||` it executed
                // because left was falsy — in either case the binding
                // is observable on the right side. Seeded BEFORE the
                // narrowing so that a left that is itself the
                // assignment narrows what it just bound (`x` is `T`
                // on the right of `(x = find_by(…)) &&`, `T?` after
                // `||`).
                let mut seeded = ctx.clone();
                collect_var_assignments_into(left, &mut seeded.local_bindings);
                self.propagate_match_bindings(left, &mut seeded, true);
                let pred = narrowing::extract_narrowing(left);
                let right_ctx = match (&pred, &*op) {
                    (Some(p), crate::expr::BoolOpKind::And) => {
                        narrowing::apply_narrowing(&seeded, p, true)
                    }
                    (Some(p), crate::expr::BoolOpKind::Or) => {
                        narrowing::apply_narrowing(&seeded, p, false)
                    }
                    _ => seeded,
                };
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
                // the rust emit's Family 4 (String → &str) fire.
                let lt_peeled = if matches!(op, crate::expr::BoolOpKind::Or) {
                    // Two `||` shapes are not unions at all, and saying
                    // union anyway is what cost campfire's spinel BINARY
                    // a link. `ActionText::ContentHelper.allowed_attributes
                    // || (…).to_a` has a left arm that can only be one
                    // thing; the union dragged the right arm's
                    // `Array[untyped]` into a list declared
                    // `Array[String]` further down.
                    //
                    //   * a left that can NEVER be falsy short-circuits
                    //     every time, so the result is exactly its type.
                    //     Ruby's falsy set is `nil` and `false` and
                    //     nothing else, so this reads off the type.
                    //   * a left that is exactly `Nil` never wins, so the
                    //     result is exactly the RIGHT arm's type —
                    //     `remove_nil` leaves a bare `Nil` alone (it peels
                    //     from a union), and the union then re-added the
                    //     nil the operator had just ruled out.
                    if never_falsy(&lt) {
                        return lt;
                    }
                    if matches!(lt, Ty::Nil) {
                        return rt;
                    }
                    narrowing::remove_nil(&lt)
                } else {
                    // `a && b` answers `a` only when `a` was falsy, so only
                    // the arms of `a` that CAN be falsy belong to the result:
                    // `params[:id] && params[:id].to_i` (which is how `&.`
                    // reads) is `Integer | nil`, not `String | Integer | nil`
                    // -- a String is never the falsy value that stopped it.
                    match falsy_part(&lt) {
                        Some(f) => f,
                        None => return rt,
                    }
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

            // `method(:name)` / `recv.method(:name)`. Unlike `&:sym`
            // (desugared to a real `Lambda` calling through
            // `block_ctx_for`'s elem-type binding), there is no body
            // here to give per-block-arg types to — the callee's arity
            // is a fact about `name`'s *definition*, not this call
            // site. So type it the same way ordinary dispatch types
            // any other call: resolve `name` on the receiver (explicit,
            // or `self` when elided) through the SAME registry lookup
            // `Send` uses, with no argument evidence (`&[]`) since none
            // is available yet — the block's yielded values become
            // ARGUMENTS to `name` at runtime, not evidence about ITS
            // return type. `block_ret` in the `Send` arm below reads
            // this node's type as the element type for map/select/etc.
            //
            // Known gap: `name`'s OWN param types come from its
            // inferred/declared signature (call sites elsewhere,
            // an RBS sig, …) — never from THIS site, since
            // `collect_send_sites`'s evidence-gathering walk only
            // reads `Send` nodes, and `&method(:name)` isn't one. A
            // helper referenced ONLY via `&method(:name)` therefore
            // types its params `Untyped` rather than the block's
            // element type — a gradual fallback (not an unresolved
            // `Var`, so invariant 1's zero-unresolved-type gate still
            // holds), just a weaker inference than `&:sym` gets. See
            // `tests/analyze.rs`'s `method_ref_block_arg_types_map_
            // result_by_referenced_method_return_ty` for the case this
            // does resolve (the common one — a helper also called
            // directly somewhere feeds its own param types).
            ExprNode::MethodRef { recv, name } => {
                let recv_ty = match recv {
                    Some(r) => Some(self.analyze_expr(r, ctx)),
                    None => ctx.self_ty.clone(),
                };
                self.dispatch(recv_ty.as_ref(), name, None, &[])
            }

            ExprNode::Apply { fun, args, block } => {
                self.analyze_expr(fun, ctx);
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                if let Some(b) = block { self.analyze_expr(b, ctx); }
                unknown()
            }

            ExprNode::Send { recv, method, args, block, parenthesized } => {
                expr.decisions &= !crate::expr::RESOLVED_DATA_FACTORY;
                if let Some(ty) = self.data_factories.and_then(|factories| factories.get(&expr_span)) {
                    // Only admitted declarations establish a Data class identity.
                    expr.decisions |= crate::expr::RESOLVED_DATA_FACTORY;
                    expr.diagnostic = None;
                    if let Some(recv) = recv { self.analyze_expr(recv, ctx); }
                    for arg in args.iter_mut() { self.analyze_expr(arg, ctx); }
                    return ty.clone();
                }
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
                    // Kernel IO — `puts`/`print`/`warn` return nil.
                    if matches!(method.as_str(), "puts" | "print" | "warn") {
                        for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                        return Ty::Nil;
                    }
                    // Kernel#format / #sprintf — a String, whatever the
                    // args (the runtime's number_with_precision builds
                    // its format string dynamically).
                    if matches!(method.as_str(), "format" | "sprintf") {
                        for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                        return Ty::Str;
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

                let mut recv_ty = match recv.as_mut() {
                    Some(r) => Some(self.analyze_expr(r, ctx)),
                    None => ctx.self_ty.clone(),
                };
                // `defined?(Foo)` keeps the written path but must not
                // resolve or autoload it. Absence is a runtime answer.
                let defined_constant = recv.is_none()
                    && method.as_str() == "defined?"
                    && args.len() == 1
                    && args[0].decisions & crate::expr::DEFINED_CONSTANT != 0;
                for a in args.iter_mut() {
                    if defined_constant {
                        let path = match &*a.node {
                            ExprNode::Const { path } => path.as_slice(),
                            _ => &[],
                        };
                        let rooted = path.first().is_some_and(|segment| segment.as_str().is_empty());
                        let id = if rooted {
                            crate::ident::ClassId(crate::ident::Symbol::from(
                                format!("::{}", path[1..].iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")).as_str(),
                            ))
                        } else {
                            written_class_id(path)
                        };
                        a.ty = Some(Ty::Class { id, args: vec![] });
                        continue;
                    }
                    self.analyze_expr(a, ctx);
                }
                if let Some(r) = recv.as_mut() {
                    if promotes_to_param_value(r, recv_ty.as_ref(), method, args, &ctx.local_bindings) {
                        r.ty = Some(send::param_value_ty());
                        recv_ty = r.ty.clone();
                    }
                }
                let block_ret = if let Some(b) = block {
                    let block_ctx = self.block_ctx_for(ctx, recv_ty.as_ref(), method, args, b);
                    let method_ref_ty = self.analyze_expr(b, &block_ctx);
                    // The Lambda walker stores the analyzed body's type
                    // on the body expr itself. `map`/`collect`/similar
                    // use that to determine the output element type.
                    // `MethodRef` (`&method(:name)`) has no body to
                    // read from — its own computed type already IS
                    // the referenced method's return type.
                    match &*b.node {
                        ExprNode::Lambda { body, .. } => body.ty.clone(),
                        ExprNode::MethodRef { .. } => Some(method_ref_ty),
                        _ => None,
                    }
                } else {
                    None
                };
                if method.as_str() == "new"
                    && matches!(&recv_ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "Data")
                    && self.const_resolver.as_ref().is_some_and(|resolver| !resolver.has_source_namespace("Data"))
                {
                    expr.diagnostic = Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("Data.new"),
                        detail: "Data is abstract; construct a class returned by Data.define".into(),
                    });
                    return unknown();
                }
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
                if let Some(t) = self.assoc_extension_ty(recv.as_ref(), method) {
                    return t;
                }
                // `x.attr = v` evaluates to `v` — Ruby's rule for an
                // attribute assignment, whatever the writer's body
                // returns. Same fact the harvest declares for a setter's
                // return; typed here too so a body that ends in one
                // (`Current.instance.session = value`) agrees with it.
                // An argument the typer could not resolve (Var) or only
                // resolved gradually (Untyped) says nothing the writer's
                // registered answer does not say better — campfire's
                // `self.join_code = generate_join_code` keeps the
                // column's String rather than the generator's untyped.
                if recv.is_some()
                    && args.len() == 1
                    && crate::analyze::is_setter_name(method)
                {
                    if let Some(t) = args[0]
                        .ty
                        .clone()
                        .filter(|t| !matches!(t, Ty::Var { .. } | Ty::Untyped))
                    {
                        return t;
                    }
                }
                // `authenticated_by.bot_key?` / `content_type.attachment?`
                // — an inquirer predicate, decided on the receiver
                // EXPRESSION (the value types as the Str it is).
                if let Some(recv) = recv.as_ref() {
                    if let Some(inquirers) = self.inquirers {
                        if crate::analyze::inquiry::is_inquiry_predicate(
                            recv, method, args, inquirers,
                        ) {
                            return Ty::Bool;
                        }
                    }
                }
                // `group(:col).count` answers a Hash, not an Int. Resolved
                // here rather than in `dispatch` because the discriminator
                // is the receiver EXPRESSION (`Ty::Relation` carries no
                // grouped state) and `dispatch` sees only its type.
                if let Some(t) = self.grouped_count_ty(
                    recv.as_ref(),
                    recv_ty.as_ref(),
                    method,
                    args,
                    block.as_ref(),
                ) {
                    return t;
                }
                if let Some(t) =
                    self.column_attribute_access_ty(recv_ty.as_ref(), method, args)
                {
                    return t;
                }
                if let Some(t) = recv.as_ref().and_then(|r| time_parse_ty(r, method, args)) {
                    return t;
                }
                if let Some(t) = expect_hash_arg_ty(recv_ty.as_ref(), method.as_str(), args) {
                    return t;
                }
                // Not `Integer?`: `nil.to_fs` raises in Rails, so only a number that cannot be nil types.
                if matches!(recv_ty, Some(Ty::Int) | Some(Ty::Float))
                    && crate::lower::number_to_fs::is_delimited_to_fs(method, args)
                {
                    return Ty::Str;
                }
                let dispatched = self.dispatch(recv_ty.as_ref(), method, block_ret.as_ref(), args);
                // Kernel.Array is a container even for scalar params.
                // App methods (including inherited/included overrides)
                // have already dispatched above and must win.
                if recv.is_none() && method.as_str() == "Array" && args.len() == 1
                    && block.is_none() && matches!(dispatched, Ty::Var { .. })
                {
                    return Ty::Array { elem: Box::new(unknown()) };
                }
                dispatched
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
                // The assignment binds first and the predicate narrows
                // what it bound: `if user = User.authenticate_by(…)`
                // reads `user` as `User` in the then-branch, `User?`
                // in the else-branch. (Narrowing before seeding would
                // find no binding to narrow and leave the nil arm.)
                let mut cond_assigns: HashMap<Symbol, Ty> = HashMap::new();
                collect_var_assignments_into(cond, &mut cond_assigns);
                let mut base = ctx.clone();
                for (k, v) in &cond_assigns {
                    base.local_bindings.insert(k.clone(), v.clone());
                }
                self.propagate_match_bindings(cond, &mut base, true);
                let then_ctx = match &pred {
                    Some(p) => narrowing::apply_narrowing(&base, p, true),
                    None => base.clone(),
                };
                let t = self.analyze_expr(then_branch, &then_ctx);
                let else_ctx = match &pred {
                    Some(p) => narrowing::apply_narrowing(&base, p, false),
                    None => base,
                };
                let e = self.analyze_expr(else_branch, &else_ctx);
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

            // Pattern bindings are lexical locals. Failed guards do not
            // roll them back, and failed patterns can bind a prefix.
            // Sibling arms see possible earlier bindings joined with
            // their old value (nil for a newly introduced local).
            ExprNode::CaseMatch { scrutinee, arms, else_body } => {
                let scrutinee_ty = self.analyze_expr(scrutinee, ctx);
                let mut branch_tys = Vec::new();
                let mut fallthrough = ctx.clone();
                for arm in arms.iter_mut() {
                    let bindings =
                        self.match_pattern_bindings(&arm.pattern, Some(&scrutinee_ty));
                    let mut inner = fallthrough.clone();
                    for (name, ty) in &bindings {
                        inner.local_bindings.insert(name.clone(), ty.clone());
                    }
                    self.analyze_match_pattern_constants(&mut arm.pattern, &inner);
                    if let Some((_, g)) = &mut arm.guard {
                        self.analyze_expr(g, &inner);
                    }
                    branch_tys.push(self.analyze_expr(&mut arm.body, &inner));
                    for (name, ty) in bindings {
                        let old = fallthrough.local_bindings.get(&name).cloned().unwrap_or(Ty::Nil);
                        fallthrough.local_bindings.insert(name, union_of(old, ty));
                    }
                }
                if let Some(eb) = else_body {
                    branch_tys.push(self.analyze_expr(eb, &fallthrough));
                }
                union_many(branch_tys)
            }

            // The predicate returns Bool; user deconstruction/pin methods
            // can still raise. Bindings escape, including partial matches.
            ExprNode::MatchPredicate { value, pattern } => {
                self.analyze_expr(value, ctx);
                self.analyze_match_pattern_constants(pattern, ctx);
                Ty::Bool
            }

            // `value => pattern` — binds on match, raises
            // `NoMatchingPatternError` otherwise; evaluates to `nil`.
            // Statement-position bindings ARE propagated forward — see
            // the `Seq` walk below, which special-cases this the same
            // way it special-cases `Assign`.
            ExprNode::MatchRequired { value, pattern } => {
                self.analyze_expr(value, ctx);
                self.analyze_match_pattern_constants(pattern, ctx);
                Ty::Nil
            }

            ExprNode::Seq { exprs } => {
                // Within a Seq, walk statements in order and thread
                // bindings forward: `@post = Post.find(...)` in stmt i
                // lets stmt i+1 resolve `@post`. Same for local vars —
                // `x = post.title` binds `x` for subsequent statements.
                let mut local_ctx = ctx.clone();
                let mut last = Ty::Nil;
                // Statement index of each live empty-array accumulator
                // seed (`parts = []`), keyed by (is_ivar, name). The push
                // refinement below updates the BINDING when the element
                // type is revealed, but emitters that must commit a decl
                // type at the seed (Go's `parts := []string{}`, Crystal's
                // `[] of String`) read the literal's stamped ty — leaving
                // it at the `Var` placeholder makes the decl and the
                // refined uses disagree (`[]interface{}` handed to
                // `strings.Join`). Recording the index lets the
                // refinement retro-stamp the seed. A later reassignment
                // of the name retires its entry.
                let mut array_seed_idx: HashMap<(bool, Symbol), usize> = HashMap::new();
                // Statement index + accumulated type of each live
                // nil-seeded LOCAL (`size = nil`). The scalar analog of
                // `array_seed_idx`, and the same decl-site hazard: a
                // later `size = v` (typically nested in a block, so the
                // Seq walk never sees it as a statement) refines the
                // binding, but the seed keeps `Ty::Nil` — and a stale
                // `Ty::Nil` is worse here than a stale element type,
                // because `nil?` const-folds on it. Go's `nil?` arm
                // maps `Some(Ty::Nil) => "true"`, so `if !size.nil?`
                // emitted `if !(true)` and the guarded branch became
                // DEAD CODE — a silent miscompile, not a type error.
                // Accumulating the union here keeps the seed, the decl
                // and every `.nil?` read in agreement.
                //
                // Locals only: the hazard is Go's `var x T = nil` decl
                // path (`emit/go/expr.rs`, LValue::Var), and ivars emit
                // through package vars / struct fields that never take a
                // seed-derived decl type.
                let mut nil_seed: HashMap<Symbol, (usize, Ty)> = HashMap::new();
                // Statement index of each live empty-hash seed (`attrs =
                // {}`) — the Hash counterpart of `array_seed_idx`.
                let mut hash_seed_idx: HashMap<(bool, Symbol), usize> = HashMap::new();
                for i in 0..exprs.len() {
                    let read_structurally_later = match &*exprs[i].node {
                        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => {
                            exprs[i + 1..].iter().any(|later| reads_structurally(later, name))
                        }
                        _ => false,
                    };
                    let e = &mut exprs[i];
                    last = self.analyze_expr(e, &local_ctx);
                    if let ExprNode::Assign { target, value } = &*e.node {
                        let slot = match target {
                            LValue::Ivar { name } => Some((true, name.clone())),
                            LValue::Var { name, .. } => Some((false, name.clone())),
                            _ => None,
                        };
                        if let Some((is_ivar, name)) = slot {
                            if let Some(ty) = e.ty.clone() {
                                if is_ivar {
                                    local_ctx.ivar_bindings.insert(name.clone(), ty);
                                } else {
                                    local_ctx.local_bindings.insert(name.clone(), ty);
                                }
                            }
                            if !is_ivar {
                                let mark = param_local_mark(&name);
                                if is_params_rooted(value, &local_ctx.local_bindings)
                                    || is_ivar_params_rooted(value, &local_ctx.local_bindings)
                                {
                                    local_ctx.local_bindings.insert(mark, Ty::Nil);
                                    // Not per use: a local the rest of the body reads as a hash or an array is one from its assignment on, so its `present?` grounds for that too.
                                    if read_structurally_later {
                                        local_ctx.local_bindings.insert(name.clone(), send::param_value_ty());
                                    }
                                } else {
                                    local_ctx.local_bindings.remove(&mark);
                                }
                            }
                            let empty_seed = matches!(&*value.node,
                                    ExprNode::Array { elements, .. } if elements.is_empty())
                                && matches!(e.ty.as_ref(),
                                    Some(Ty::Array { elem }) if matches!(&**elem, Ty::Var { .. }));
                            if empty_seed {
                                array_seed_idx.insert((is_ivar, name.clone()), i);
                            } else {
                                array_seed_idx.remove(&(is_ivar, name.clone()));
                            }
                            // Same bookkeeping for the `h = {}` seed.
                            let empty_hash_seed = matches!(&*value.node,
                                    ExprNode::Hash { entries, .. } if entries.is_empty())
                                && matches!(e.ty.as_ref(),
                                    Some(Ty::Hash { value, .. })
                                        if matches!(&**value, Ty::Var { .. }));
                            if empty_hash_seed {
                                hash_seed_idx.insert((is_ivar, name.clone()), i);
                            } else {
                                hash_seed_idx.remove(&(is_ivar, name.clone()));
                            }
                            // `x = nil` opens a scalar accumulator.
                            // Unlike the array seed, a later top-level
                            // reassignment does NOT retire it: `x = nil;
                            // x = "s"` still needs a decl type spanning
                            // both, and the refinement below unions the
                            // reassignment in (it walks this statement's
                            // subtree, which includes the statement
                            // itself). Only locals participate.
                            if !is_ivar
                                && matches!(&*value.node,
                                    ExprNode::Lit { value: Literal::Nil })
                                && matches!(e.ty.as_ref(), Some(Ty::Nil))
                                && !nil_seed.contains_key(&name)
                            {
                                nil_seed.insert(name, (i, Ty::Nil));
                            }
                        }
                    }
                    // A `begin … rescue … end` STATEMENT exports the
                    // locals its body and rescue arms assign at their own
                    // top level — Ruby's `begin` opens no scope — typed
                    // as the assigned value OR what the name held before
                    // (nil when it was unbound): the exception may come
                    // before the assignment runs. Without this a local
                    // assigned in one `begin` and read after it (`user =
                    // User.find(id)` in a begin, `user.messages.create!`
                    // below) read as unresolved, and everything keyed on
                    // its type — the association constructor rewrite —
                    // silently declined. Found by the model scenario
                    // scripts/campfire-db-differential runs statement by
                    // statement.
                    if let ExprNode::BeginRescue { body, rescues, .. } = &*e.node {
                        let mut exported: Vec<(Symbol, Ty)> = Vec::new();
                        top_level_local_assigns(body, &mut exported);
                        for rc in rescues.iter() {
                            top_level_local_assigns(&rc.body, &mut exported);
                        }
                        for (name, ty) in exported {
                            let prior = local_ctx.local_bindings.get(&name).cloned().unwrap_or(Ty::Nil);
                            local_ctx.local_bindings.insert(name, union_of(prior, ty));
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
                    self.propagate_match_bindings(e, &mut local_ctx, true);
                    // Container element write — `hash[k] ||= []` /
                    // `hash[k] = v`. Widen the container's value type so
                    // a following `hash[k].push x` / `.join` in the same
                    // scope reads `Array|Nil` (resolvable via union
                    // dispatch) instead of `Var|Nil`. An empty `{}` seeds
                    // the value as Var; the accumulator idiom only
                    // reveals the element type at the write. The `{}` is
                    // bound on the recv ivar/local; a computed recv
                    // (`a.b[k]`) has no binding to refine, so skip it.
                    //
                    // Subtree-wide, for the reason spelled out on the
                    // array block below: the accumulator's writes live
                    // in a nested block (`opts.each { |k, v| attrs[k] =
                    // v }`) while its `{}` seed sits at statement level.
                    // A same-level match saw only the top-level writes,
                    // which is worse than seeing none — the visible
                    // `attrs[:src] = <Str>` monomorphized the hash while
                    // the invisible `attrs[k] = <Untyped>` still emitted
                    // against it (`interface{}` into `map[string]string`).
                    // Partial write visibility is unsound; whole-subtree
                    // visibility is what makes the refinement a fact.
                    let mut hash_writes: Vec<(bool, Symbol, Ty)> = Vec::new();
                    collect_hash_index_writes(e, &mut hash_writes);
                    // Array accumulator writes anywhere in this statement —
                    // `arr << x` / `arr.push(x)`, including nested in a loop,
                    // branch, or block (`while { acc << x }`, `items.each {
                    // acc << x }`). The Array analog of the Hash `h[k]=v`
                    // block above, but subtree-wide: the accumulator idiom
                    // fills a nested loop/block while the `arr = []` seed sits
                    // at the top level, so a same-level-only match (like the
                    // Hash one) would miss it — `analyze_expr(e)` above has
                    // already stamped every pushed arg's `.ty`, so a subtree
                    // scan can read them. Widen the outer-bound array's
                    // element from its empty-literal `Var` placeholder (or
                    // union the observed element into an existing one). Only
                    // bindings that already exist as `Array[_]` are touched.
                    let mut pushes: Vec<(bool, Symbol, Ty)> = Vec::new();
                    collect_array_pushes(e, &mut pushes);
                    // Apply the hash writes collected above. Deferred to
                    // here so both collections finish while the `&mut
                    // exprs[i]` borrow is live, leaving it dead before
                    // the seed retro-stamps reach back into `exprs`.
                    for (is_ivar, name, rhs) in hash_writes {
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
                            // Key stays as-is: the observed index
                            // expressions mix `:src` (Sym) with a block
                            // param (Untyped), and unioning those buys
                            // nothing — every target renders Ruby hash
                            // keys as strings regardless.
                            let refined = Ty::Hash {
                                key: key.clone(),
                                value: Box::new(new_value),
                            };
                            bindings.insert(name.clone(), refined.clone());
                            // Retro-stamp the `{}` seed, same as the
                            // array case below. This is the gap the
                            // array fix (b79e7fd5) called out as still
                            // latent for hashes: without it the seed
                            // keeps `Hash[Var, Var]`, and Go's empty-
                            // literal fallback guesses `map[string]
                            // string` — a guess that silently disagrees
                            // with every non-string write.
                            //
                            // …but only for a NON-nilable observation.
                            // `params[name] = path_parts[i]` observes
                            // `Str | Nil` because Ruby's `Array#[]` can
                            // return nil — a fact the emitted code does
                            // not share (Crystal's `Array(String)#[]`
                            // raises rather than returning nil, and the
                            // other targets index the same way). Stamping
                            // that onto the seed contradicts the emit
                            // instead of describing it, and outranks the
                            // author's declared `Hash[String, String]`
                            // return: Crystal rejected `match_parts` for
                            // returning `Hash(String, String | Nil)`.
                            // The placeholder is the better answer there
                            // — it lets each target's own declaration or
                            // default stand.
                            let nilable = match &refined {
                                Ty::Hash { value, .. } => match &**value {
                                    Ty::Nil => true,
                                    Ty::Union { variants } => {
                                        variants.iter().any(|t| matches!(t, Ty::Nil))
                                    }
                                    _ => false,
                                },
                                _ => false,
                            };
                            if let Some(&si) = hash_seed_idx.get(&(is_ivar, name)) {
                                if !nilable {
                                    let seed = &mut exprs[si];
                                    if let ExprNode::Assign { value, .. } = &mut *seed.node {
                                        value.ty = Some(refined.clone());
                                    }
                                    seed.ty = Some(refined);
                                }
                            }
                        }
                    }
                    for (is_ivar, name, elem) in pushes {
                        let bindings = if is_ivar {
                            &mut local_ctx.ivar_bindings
                        } else {
                            &mut local_ctx.local_bindings
                        };
                        if let Some(Ty::Array { elem: cur }) = bindings.get(&name) {
                            let new_elem = if matches!(**cur, Ty::Var { .. }) {
                                elem
                            } else {
                                union_of((**cur).clone(), elem)
                            };
                            let refined = Ty::Array { elem: Box::new(new_elem) };
                            bindings.insert(name.clone(), refined.clone());
                            // Retro-stamp the seed literal so decl-site
                            // emitters agree with the refined uses: the
                            // binding refinement alone left Go declaring
                            // `[]interface{}{}` while a later `.join`
                            // (typed off the refined binding) emitted
                            // `strings.Join` over it. Each subsequent
                            // push re-stamps with the widened union.
                            if let Some(&si) = array_seed_idx.get(&(is_ivar, name)) {
                                let seed = &mut exprs[si];
                                if let ExprNode::Assign { value, .. } = &mut *seed.node {
                                    value.ty = Some(refined.clone());
                                }
                                seed.ty = Some(refined);
                            }
                        }
                    }
                    // Nil-seeded scalar refinement — the `size = nil` …
                    // `size = v` pair, where the write is usually nested
                    // in a block and so invisible to this statement walk.
                    // Subtree-wide for the same reason `collect_array_
                    // pushes` is. Each observed write unions into the
                    // seed's running type and retro-stamps the seed
                    // (Assign + literal), so the decl, the binding and
                    // every later `.nil?` read all resolve to the same
                    // `Union[Nil, T]` instead of the seed's bare `Nil`.
                    if !nil_seed.is_empty() {
                        let mut writes: Vec<(Symbol, Ty)> = Vec::new();
                        collect_local_assignment_tys(&exprs[i], &mut writes);
                        for (name, ty) in writes {
                            let Some((si, acc)) = nil_seed.get(&name) else {
                                continue;
                            };
                            let (si, merged) = (*si, union_of(acc.clone(), ty));
                            if &merged == acc {
                                continue;
                            }
                            nil_seed.insert(name.clone(), (si, merged.clone()));
                            local_ctx
                                .local_bindings
                                .insert(name.clone(), merged.clone());
                            let seed = &mut exprs[si];
                            if let ExprNode::Assign { value, .. } = &mut *seed.node {
                                value.ty = Some(merged.clone());
                            }
                            seed.ty = Some(merged);
                        }
                    }
                    let e = &exprs[i];
                    // A conditional/loop whose CONDITION assigns a local
                    // (`if !(story = find) || story.gone?`, `while (line =
                    // gets)`) binds that local for the statements that
                    // FOLLOW the construct: the condition always
                    // evaluates, so the assignment ran regardless of which
                    // branch/iteration executed. The If/While handlers
                    // thread these into their own branches only — without
                    // this the pervasive find-or-return guard idiom leaves
                    // the local `Var` for every later read (and its
                    // `.build`/attribute cascade). Threaded BEFORE the
                    // diverging-then narrowing below so that block can
                    // further tighten the now-bound local to non-nil on
                    // the fall-through path. A Var/Bottom RHS is skipped so
                    // a good prior binding isn't clobbered.
                    if let ExprNode::If { cond, .. } | ExprNode::While { cond, .. } =
                        &*e.node
                    {
                        let mut cond_assigns: HashMap<Symbol, Ty> = HashMap::new();
                        collect_var_assignments_into(cond, &mut cond_assigns);
                        for (name, ty) in cond_assigns {
                            if ty.is_open() {
                                continue;
                            }
                            local_ctx.local_bindings.insert(name, ty);
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

            ExprNode::Retry | ExprNode::Redo => {
                // `retry` re-runs the enclosing begin/rescue; `redo`
                // re-runs the current loop/block iteration. Both carry no
                // value and divert control, so they type as `Bottom` and
                // drop out of joins just like `next` / `break`.
                Ty::Bottom
            }

            ExprNode::ForwardArgs => Ty::Untyped,

            ExprNode::Splat { value } | ExprNode::KeywordSplat { value } => {
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
        // A regex literal is a `Regexp`, and saying so is not a
        // refinement — it is the difference between `/Android/` being a
        // known object and being a gradual hole. Every `match?(/x/)` in
        // `runtime/ruby/` passed an untyped argument until now, and
        // `runtime/ruby/` prices every target.
        //
        // `Regexp` is the same kind of name `Time` is: the class exists
        // in the registry (`stdlib`), so a call ON the literal resolves
        // through the ordinary class walk rather than through a
        // per-literal table.
        Literal::Regex { .. } => {
            Ty::Class { id: ClassId(Symbol::from("Regexp")), args: vec![] }
        }
    }
}

/// The class path as written, joined with `::`.
fn written_class_id(path: &[Symbol]) -> ClassId {
    if let [name] = path {
        return ClassId(name.clone());
    }
    let mut name = String::new();
    for part in path {
        if !name.is_empty() {
            name.push_str("::");
        }
        name.push_str(part.as_str());
    }
    ClassId(Symbol::from(name))
}

/// Rubydex can resolve a relative path to a name with a lexical prefix:
/// `RateCalculator` inside `module PriceSupport` is
/// `PriceSupport::RateCalculator`. Ingest copies a concern's methods
/// into each class that includes it, and an emitter can nest a
/// declaration in another way. Thus the IR keeps the qualified path,
/// which names the same class from every lexical scope. A rooted path
/// (`::A`) already names its class. A path that the resolved name does
/// not end with (a constant alias) stays as written.
fn qualify_resolved_path(path: &mut Vec<Symbol>, resolved: &ClassId) {
    if path.first().is_none_or(|head| head.as_str().starts_with("::")) {
        return;
    }
    let mut prefix = resolved.0.as_str();
    for segment in path.iter().rev() {
        let Some(rest) = prefix
            .strip_suffix(segment.as_str())
            .and_then(|rest| rest.strip_suffix("::"))
        else {
            return;
        };
        prefix = rest;
    }
    if !prefix.is_empty() {
        *path = resolved.0.as_str().split("::").map(Symbol::from).collect();
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
    let peeled = expected.peel_nilable();
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
        // Destructuring a relation materializes it — each scalar
        // target gets the element model, same as `Array<of>`.
        Some(Ty::Relation { of }) => Some(Ty::Class { id: of.clone(), args: vec![] }),
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
/// Collect array-accumulator pushes (`arr << x`, `arr.push/append/
/// unshift/prepend(x, …)`) anywhere in `expr`, as `(is_ivar, name,
/// element_ty)` per push whose receiver is a plain local or ivar.
/// Recurses through loop/branch bodies and blocks (via `for_each_child`)
/// so the accumulator idiom — seed `arr = []` at the top, fill it inside
/// a nested `while`/`each` — is caught. `concat` is excluded: it flattens
/// its argument, so the pushed element type isn't the argument's type.
/// A pushed arg typed `Var`/`Bottom` contributes nothing. Callers refine
/// only bindings that already exist as `Array[_]`, so an inner shadowing
/// local of the same name at worst adds imprecision, never a wrong write.
fn collect_array_pushes(expr: &Expr, out: &mut Vec<(bool, Symbol, Ty)>) {
    if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*expr.node {
        if matches!(method.as_str(), "<<" | "push" | "append" | "unshift" | "prepend") {
            let slot = match &*recv.node {
                ExprNode::Ivar { name } => Some((true, name.clone())),
                ExprNode::Var { name, .. } => Some((false, name.clone())),
                _ => None,
            };
            if let Some((is_ivar, name)) = slot {
                let mut pushed: Option<Ty> = None;
                for a in args {
                    let Some(t) = a.ty.clone() else { continue };
                    if t.is_open() {
                        continue;
                    }
                    pushed = Some(match pushed.take() {
                        Some(p) => union_of(p, t),
                        None => t,
                    });
                }
                if let Some(t) = pushed {
                    out.push((is_ivar, name, t));
                }
            }
        }
    }
    expr.node.for_each_child(&mut |child| collect_array_pushes(child, out));
}

/// Every container index-write in `expr`'s subtree whose receiver is a
/// plain ivar/local binding, as `(is_ivar, name, written_ty)`. Covers
/// both `h[k] = v` and `h[k] ||= v`.
///
/// A computed receiver (`a.b[k] = v`) has no binding to refine and is
/// skipped. Open written types are skipped for the same reason as in
/// [`collect_array_pushes`] — they carry nothing to union in.
fn collect_hash_index_writes(expr: &Expr, out: &mut Vec<(bool, Symbol, Ty)>) {
    let write = match &*expr.node {
        ExprNode::Assign { target: LValue::Index { recv, .. }, value }
        | ExprNode::OpAssign { target: LValue::Index { recv, .. }, value, .. } => {
            value.ty.clone().map(|t| (recv, t))
        }
        // `h[k] = v` reaches the analyzer as a two-arg `[]=` send, NOT
        // as an `LValue::Index` assign — the LValue shape only survives
        // for the compound `h[k] ||= v`. Matching just the LValue (as
        // this refinement originally did) therefore missed the plain
        // write, which is the form the accumulator idiom actually uses.
        ExprNode::Send { recv: Some(r), method, args, .. }
            if matches!(method.as_str(), "[]=" | "store") && args.len() == 2 =>
        {
            args[1].ty.clone().map(|t| (r, t))
        }
        _ => None,
    };
    if let Some((recv, rhs)) = write {
        if !rhs.is_open() {
            let slot = match &*recv.node {
                ExprNode::Ivar { name } => Some((true, name.clone())),
                ExprNode::Var { name, .. } => Some((false, name.clone())),
                _ => None,
            };
            if let Some((is_ivar, name)) = slot {
                out.push((is_ivar, name, rhs));
            }
        }
    }
    expr.node.for_each_child(&mut |child| collect_hash_index_writes(child, out));
}

/// Every local-variable assignment in `expr`'s subtree that carries a
/// usable stamped type, as `(name, assigned_ty)`. The scalar counterpart
/// of [`collect_array_pushes`]: the nil-accumulator idiom writes from
/// inside a block (`opts.each { |k, v| size = v }`) while the `size =
/// nil` seed sits at the enclosing statement level, so a same-level
/// match would never see the write.
///
/// Distinct from [`collect_var_assignments_into`], which walks only the
/// condition-expression node shapes and keeps the LAST write per name;
/// this one is a full subtree walk and reports EVERY write, because the
/// caller unions them rather than overwriting.
///
/// Open types (`Var` / `Bottom`) are skipped — an unresolved write
/// carries no information to union in, and folding it would erase a
/// good type. `Untyped` is deliberately NOT skipped: it is an
/// author-signed "no constraint here", and a hash value flowing into
/// the accumulator must widen the seed rather than leave it narrow.
fn collect_local_assignment_tys(expr: &Expr, out: &mut Vec<(Symbol, Ty)>) {
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*expr.node {
        if let Some(t) = value.ty.clone() {
            if !t.is_open() {
                out.push((name.clone(), t));
            }
        }
    }
    expr.node.for_each_child(&mut |child| collect_local_assignment_tys(child, out));
}

/// The local assignments a statement list makes at ITS OWN level — the
/// statement itself, or each statement of a `Seq` — with the value's
/// stamped type. Blocks and nested bodies are not entered: a local first
/// assigned inside a block does not outlive it. Open types are skipped,
/// as in [`collect_local_assignment_tys`].
fn top_level_local_assigns(expr: &Expr, out: &mut Vec<(Symbol, Ty)>) {
    let stmts: Vec<&Expr> = match &*expr.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![expr],
    };
    for s in stmts {
        if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*s.node {
            if let Some(t) = value.ty.clone() {
                if !t.is_open() {
                    out.push((name.clone(), t));
                }
            }
        }
    }
}

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
    //
    // The result is CANONICAL: at most one Hash and one Array spine
    // (merged pointwise, so the structural join above also fires when
    // a container arrives wrapped in a Union), variants in a fixed
    // sort order. Canonical form is what makes `union_of` commutative
    // and associative under derived `==` — fixpoint convergence checks
    // and iteration order both depend on that; the lattice-law tests
    // in this file's test module enforce it.
    let mut variants = Vec::new();
    push_union_variants(a, &mut variants);
    push_union_variants(b, &mut variants);
    Ty::canonicalize_variants(&mut variants);
    match variants.len() {
        0 => Ty::Bottom,
        1 => variants.into_iter().next().unwrap(),
        _ => Ty::Union { variants },
    }
}

/// Append `t`'s constituent variants to `out`, flattening nested
/// `Union`s and skipping any variant already present (structural
/// equality) so the result carries each distinct type once. A `Hash`
/// or `Array` merges pointwise into an existing same-spine variant
/// instead of appending, keeping the union to one spine per container
/// kind regardless of the order joins arrive in.
fn push_union_variants(t: Ty, out: &mut Vec<Ty>) {
    match t {
        Ty::Union { variants } => {
            for v in variants {
                push_union_variants(v, out);
            }
        }
        Ty::Hash { key, value } => {
            for existing in out.iter_mut() {
                if let Ty::Hash { key: k0, value: v0 } = existing {
                    *k0 = Box::new(union_of((**k0).clone(), *key));
                    *v0 = Box::new(union_of((**v0).clone(), *value));
                    return;
                }
            }
            out.push(Ty::Hash { key, value });
        }
        Ty::Array { elem } => {
            for existing in out.iter_mut() {
                if let Ty::Array { elem: e0 } = existing {
                    *e0 = Box::new(union_of((**e0).clone(), *elem));
                    return;
                }
            }
            out.push(Ty::Array { elem });
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
    fn kernel_array_does_not_capture_inherited_or_included_app_methods() {
        let owner = ClassId(Symbol::from("Owner"));
        let child = ClassId(Symbol::from("Child"));
        for included in [false, true] {
            let mut classes = empty_classes();
            let mut info = ClassInfo::default();
            info.instance_methods.insert(Symbol::from("Array"), Ty::Str);
            classes.insert(owner.clone(), info);
            let mut info = ClassInfo::default();
            if included {
                info.includes.push(owner.clone());
            } else {
                info.parent = Some(owner.clone());
            }
            classes.insert(child.clone(), info);
            let mut ctx = Ctx::default();
            ctx.self_ty = Some(Ty::Class { id: child.clone(), args: vec![] });
            let mut expr = send(None, "Array", vec![nil_lit()]);
            assert_eq!(BodyTyper::new(&classes).analyze_expr(&mut expr, &ctx), Ty::Str);
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
    fn assignment_in_condition_binds_then_narrows() {
        // `if x = y then x else x end` with `y: String?` and `x`
        // unbound — the authentication generator's
        // `if user = User.authenticate_by(…)`. The then-branch reads
        // `x` as String (assigned, then truthy), the else-branch as
        // String? (assigned, not narrowed: the falsy side is left
        // alone, see `apply_narrowing`). The same for `(x = y) && x`.
        let assign = || {
            synth(ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from("x") },
                value: var("y"),
            })
        };
        let mut if_expr = synth(ExprNode::If {
            cond: assign(),
            then_branch: var("x"),
            else_branch: var("x"),
        });
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = ctx_with_local("y", optional_str());
        typer.analyze_expr(&mut if_expr, &ctx);
        let ExprNode::If { then_branch, else_branch, .. } = &*if_expr.node else {
            panic!("expected If");
        };
        assert_eq!(then_branch.ty, Some(Ty::Str), "then-branch: assigned, then narrowed");
        assert_eq!(else_branch.ty, Some(optional_str()), "else-branch: assigned, not narrowed");

        let mut and_expr = synth(ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: Default::default(),
            left: assign(),
            right: var("x"),
        });
        typer.analyze_expr(&mut and_expr, &ctx);
        let ExprNode::BoolOp { right, .. } = &*and_expr.node else {
            panic!("expected BoolOp");
        };
        assert_eq!(right.ty, Some(Ty::Str), "right of `&&`: assigned, then narrowed");
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
        // spurious .to_string().unwrap() chains (E0599 on rust's
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
        synth(ExprNode::Lambda { rest_param: None,
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
    fn array_accumulator_element_refined_by_shovel() {
        // `ordered = []; ordered << 5; ordered` should read `Array[Int]`,
        // not `Array[untyped]` — the empty-literal accumulator's element
        // type is revealed at the `<<` push. Mirrors the Hash `h[k]=v`
        // accumulator refinement. This is the arrange_for_user shape that
        // was leaving the comment-threading collection untyped.
        let assign = synth(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("ordered") },
            value: synth(ExprNode::Array { elements: vec![], style: Default::default() }),
        });
        let five = synth(ExprNode::Lit { value: Literal::Int { value: 5 } });
        let push = send(Some(var("ordered")), "<<", vec![five]);
        let read = var("ordered");
        let mut seq =
            synth(ExprNode::Seq { exprs: vec![assign, push, read] });

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = Ctx::default();
        let ty = typer.analyze_expr(&mut seq, &ctx);
        assert_eq!(ty, Ty::Array { elem: Box::new(Ty::Int) });
    }

    #[test]
    fn array_accumulator_seed_literal_retro_stamped() {
        // The refinement must reach the SEED literal's stamped ty, not
        // just the forward binding: decl-site emitters (Go `parts :=
        // []string{}`, Crystal `[] of String`) type the declaration
        // from the literal, while later uses (`.join`) type off the
        // refined binding — a binding-only refinement made Go declare
        // `[]interface{}{}` and hand it to `strings.Join([]string)`.
        let assign = synth(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("ordered") },
            value: synth(ExprNode::Array { elements: vec![], style: Default::default() }),
        });
        let five = synth(ExprNode::Lit { value: Literal::Int { value: 5 } });
        let push = send(Some(var("ordered")), "<<", vec![five]);
        let read = var("ordered");
        let mut seq =
            synth(ExprNode::Seq { exprs: vec![assign, push, read] });

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = Ctx::default();
        typer.analyze_expr(&mut seq, &ctx);

        let expected = Ty::Array { elem: Box::new(Ty::Int) };
        let ExprNode::Seq { exprs } = &*seq.node else { panic!("seq") };
        assert_eq!(exprs[0].ty.as_ref(), Some(&expected));
        let ExprNode::Assign { value, .. } = &*exprs[0].node else { panic!("assign") };
        assert_eq!(value.ty.as_ref(), Some(&expected));
    }

    #[test]
    fn nil_seeded_local_refined_by_nested_write() {
        // `size = nil; [..].each { size = <Str> }` must leave `size`
        // reading `Str | Nil`, NOT the seed's bare `Nil`. A stale
        // `Ty::Nil` is not merely imprecise: Go's `nil?` emit folds a
        // `Ty::Nil` receiver to the literal `true`, so a following
        // `if !size.nil?` became `if !(true)` and its body was dropped
        // as dead code — a silent miscompile, not a type error.
        let seed = synth(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("size") },
            value: nil_lit(),
        });
        let str_lit = synth(ExprNode::Lit {
            value: Literal::Str { value: "16x16".into() },
        });
        let inner = synth(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("size") },
            value: str_lit,
        });
        // The write is nested inside a block, which is exactly why a
        // statement-level-only walk missed it.
        let each = send(Some(var("items")), "each", vec![lambda(vec!["x"], inner)]);
        let read = var("size");
        let mut seq = synth(ExprNode::Seq { exprs: vec![seed, each, read] });

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = Ctx::default();
        let ty = typer.analyze_expr(&mut seq, &ctx);

        let expected = union_of(Ty::Nil, Ty::Str);
        assert_eq!(ty, expected, "read of the refined local");
        // …and the SEED must agree, so decl-site emitters don't declare
        // from a type the rest of the body contradicts.
        let ExprNode::Seq { exprs } = &*seq.node else { panic!("seq") };
        assert_eq!(exprs[0].ty.as_ref(), Some(&expected), "seed stmt");
        let ExprNode::Assign { value, .. } = &*exprs[0].node else { panic!("assign") };
        assert_eq!(value.ty.as_ref(), Some(&expected), "seed literal");
    }

    #[test]
    fn hash_accumulator_refined_by_nested_index_write() {
        // `attrs = {}; [..].each { attrs[k] = <Untyped> }; attrs[:src] =
        // <Str>` must widen the seed to hold BOTH writes. The nested one
        // arrives as a two-arg `[]=` send (not an `LValue::Index`), and
        // missing it was worse than seeing nothing: the visible String
        // write alone monomorphized the hash to `map[string]string`,
        // which the invisible untyped write then failed to satisfy.
        let seed = synth(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("attrs") },
            value: synth(ExprNode::Hash { entries: vec![], kwargs: false }),
        });
        // The block params take their types from the iterated hash, so
        // `v` reads `Untyped` the way `opts.to_h.each` does in the real
        // helper. `opts` has to be a real ctx binding — a ty stamped on
        // the node is overwritten by the Var lookup during analysis.
        let nested_write = send(Some(var("attrs")), "[]=", vec![var("k"), var("v")]);
        let each = synth(ExprNode::Send {
            recv: Some(var("opts")),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(lambda(vec!["k", "v"], nested_write)),
            parenthesized: true,
        });
        let str_val = synth(ExprNode::Lit {
            value: Literal::Str { value: "/a.png".into() },
        });
        let sym_key = synth(ExprNode::Lit {
            value: Literal::Sym { value: "src".into() },
        });
        let top_write = send(Some(var("attrs")), "[]=", vec![sym_key, str_val]);
        let read = var("attrs");
        let mut seq = synth(ExprNode::Seq {
            exprs: vec![seed, each, top_write, read],
        });

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = ctx_with_local(
            "opts",
            Ty::Hash {
                key: Box::new(Ty::Str),
                value: Box::new(Ty::Untyped),
            },
        );
        typer.analyze_expr(&mut seq, &ctx);

        let ExprNode::Seq { exprs } = &*seq.node else { panic!("seq") };
        let Some(Ty::Hash { value, .. }) = exprs[0].ty.as_ref() else {
            panic!("seed stamped as a Hash, got {:?}", exprs[0].ty)
        };
        // Both writes are represented — the value is no longer the
        // placeholder, and it is not the String write alone.
        assert!(
            !matches!(**value, Ty::Var { .. }),
            "seed value still the placeholder: {value:?}"
        );
        assert_ne!(**value, Ty::Str, "monomorphized to the visible write only");
    }

    #[test]
    fn nilable_hash_write_leaves_seed_placeholder() {
        // The counterweight to the test above: `params = {}; params[k] =
        // <Str | Nil>` must NOT stamp the seed. Ruby's `Array#[]` types
        // as `Str | Nil`, but no target emits it that way (Crystal's
        // `Array(String)#[]` raises rather than returning nil), so
        // stamping it contradicts the emit and outranks the author's
        // declared `Hash[String, String]` return — Crystal rejected
        // `Router.match_parts` for returning `Hash(String, String | Nil)`.
        let seed = synth(ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("params") },
            value: synth(ExprNode::Hash { entries: vec![], kwargs: false }),
        });
        let nilable = {
            let mut e = var("ap");
            e.ty = Some(optional_str());
            e
        };
        let write = send(Some(var("params")), "[]=", vec![var("name"), nilable]);
        let mut seq = synth(ExprNode::Seq { exprs: vec![seed, write] });

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = Ctx::default();
        typer.analyze_expr(&mut seq, &ctx);

        let ExprNode::Seq { exprs } = &*seq.node else { panic!("seq") };
        let Some(Ty::Hash { value, .. }) = exprs[0].ty.as_ref() else {
            panic!("seed is a Hash, got {:?}", exprs[0].ty)
        };
        assert!(
            matches!(**value, Ty::Var { .. }),
            "nilable write must leave the placeholder, got {value:?}"
        );
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

    // ── union_of lattice laws ────────────────────────────────────────
    //
    // `union_of` is THE type join: every fixpoint pass folds observed
    // types through it, and fixpoint convergence checks compare joins
    // with derived `==`. For that comparison to be semantic — and for
    // join results to be independent of the order call sites happen to
    // be visited in — the join must be commutative, associative, and
    // produce one canonical form per set of variants. These tests
    // enumerate a bounded universe of Ty shapes (chosen to cover every
    // special case inside union_of: Bottom filtering, container-spine
    // merging, nested-union flattening, dedup) and check the laws over
    // every pair and triple exhaustively.

    fn law_universe() -> Vec<Ty> {
        let class = |name: &str| Ty::Class {
            id: ClassId(Symbol::from(name)),
            args: vec![],
        };
        let arr = |elem: Ty| Ty::Array { elem: Box::new(elem) };
        let hash = |k: Ty, v: Ty| Ty::Hash {
            key: Box::new(k),
            value: Box::new(v),
        };
        vec![
            Ty::Int,
            Ty::Str,
            Ty::Time,
            Ty::Nil,
            Ty::Untyped,
            Ty::Bottom,
            Ty::Var { var: TyVar(7) },
            class("Story"),
            class("User"),
            Ty::Relation { of: ClassId(Symbol::from("Story")) },
            Ty::Relation { of: ClassId(Symbol::from("User")) },
            arr(Ty::Int),
            arr(Ty::Str),
            arr(Ty::Var { var: TyVar(3) }),
            hash(Ty::Str, Ty::Int),
            hash(Ty::Sym, Ty::Str),
            hash(
                Ty::Var { var: TyVar(1) },
                Ty::Var { var: TyVar(2) },
            ),
            Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
            Ty::Union { variants: vec![Ty::Int, Ty::Str] },
            Ty::Union {
                variants: vec![hash(Ty::Str, Ty::Int), Ty::Nil],
            },
        ]
    }

    #[test]
    fn union_of_bottom_is_identity() {
        for t in law_universe() {
            assert_eq!(union_of(Ty::Bottom, t.clone()), t, "Bottom ⊔ {t:?}");
            assert_eq!(union_of(t.clone(), Ty::Bottom), t, "{t:?} ⊔ Bottom");
        }
    }

    #[test]
    fn union_of_is_idempotent() {
        for t in law_universe() {
            assert_eq!(union_of(t.clone(), t.clone()), t, "{t:?} ⊔ itself");
        }
    }

    #[test]
    fn union_of_is_commutative() {
        for a in law_universe() {
            for b in law_universe() {
                assert_eq!(
                    union_of(a.clone(), b.clone()),
                    union_of(b.clone(), a.clone()),
                    "{a:?} ⊔ {b:?} must equal {b:?} ⊔ {a:?}",
                );
            }
        }
    }

    #[test]
    fn union_of_is_associative() {
        let universe = law_universe();
        for a in &universe {
            for b in &universe {
                for c in &universe {
                    assert_eq!(
                        union_of(union_of(a.clone(), b.clone()), c.clone()),
                        union_of(a.clone(), union_of(b.clone(), c.clone())),
                        "({a:?} ⊔ {b:?}) ⊔ {c:?} must equal {a:?} ⊔ ({b:?} ⊔ {c:?})",
                    );
                }
            }
        }
    }

    #[test]
    fn union_of_absorbs_existing_variants() {
        // a ⊔ b then re-joining either operand must be a no-op —
        // this is what lets the fixpoint detect convergence.
        for a in law_universe() {
            for b in law_universe() {
                let joined = union_of(a.clone(), b.clone());
                assert_eq!(
                    union_of(joined.clone(), a.clone()),
                    joined,
                    "({a:?} ⊔ {b:?}) ⊔ {a:?} must absorb",
                );
                assert_eq!(
                    union_of(joined.clone(), b.clone()),
                    joined,
                    "({a:?} ⊔ {b:?}) ⊔ {b:?} must absorb",
                );
            }
        }
    }

    #[test]
    fn union_of_relations_has_no_cross_model_subtyping() {
        // `Relation{of: A} ⊔ Relation{of: A}` is itself (structural
        // equality); differing models join to a flat Union — the
        // lattice must NOT invent subtyping across models (no merged
        // spine like the Hash/Array pointwise join: two relations
        // over different models are genuinely distinct results).
        let rel = |name: &str| Ty::Relation { of: ClassId(Symbol::from(name)) };
        assert_eq!(union_of(rel("Story"), rel("Story")), rel("Story"));
        match union_of(rel("Story"), rel("Comment")) {
            Ty::Union { variants } => {
                assert_eq!(variants.len(), 2, "distinct models stay distinct variants");
                assert!(variants.iter().all(|v| matches!(v, Ty::Relation { .. })));
            }
            other => panic!("expected a union, got {other:?}"),
        }
    }

    #[test]
    fn union_of_merges_container_spines_through_unions() {
        // Regression pin for the associativity break: the Hash/Hash
        // structural join must fire even when one side arrives wrapped
        // in a Union (the `(H₁ ⊔ Nil) ⊔ H₂` association order). A flat
        // union carrying two Hash spines forces every consumer to fan
        // out per variant — union_of's own doc names that shape as the
        // harmful one.
        let h1 = Ty::Hash {
            key: Box::new(Ty::Str),
            value: Box::new(Ty::Int),
        };
        let h2 = Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(Ty::Str),
        };
        let via_nil_first = union_of(union_of(h1.clone(), Ty::Nil), h2.clone());
        let via_hashes_first = union_of(union_of(h1.clone(), h2.clone()), Ty::Nil);
        assert_eq!(via_nil_first, via_hashes_first);

        let spines = match &via_nil_first {
            Ty::Union { variants } => variants
                .iter()
                .filter(|v| matches!(v, Ty::Hash { .. }))
                .count(),
            other => panic!("expected a union, got {other:?}"),
        };
        assert_eq!(spines, 1, "hash spines must merge, got {via_nil_first:?}");
    }

    #[test]
    fn concrete_value_shapes_are_truthy_for_boolean_operators() {
        let values = [
            Ty::Date,
            Ty::Tuple { elems: vec![Ty::Int] },
            Ty::Record { row: Row::default() },
            Ty::Fn {
                params: Vec::new(), block: None, ret: Box::new(Ty::Str),
                effects: crate::effect::EffectSet::pure(),
            },
        ];
        for left in values {
            assert!(never_falsy(&left), "{left:?} must short-circuit `||`");
            assert_eq!(falsy_part(&left), None, "{left:?} must yield the right arm of `&&`");
        }
    }

    #[test]
    fn nil_arms_of_concrete_value_unions_remain_falsy() {
        for truthy in [
            Ty::Date,
            Ty::Tuple { elems: vec![Ty::Int] },
            Ty::Record { row: Row::default() },
            Ty::Fn {
                params: Vec::new(), block: None, ret: Box::new(Ty::Str),
                effects: crate::effect::EffectSet::pure(),
            },
        ] {
            let left = Ty::Union { variants: vec![truthy, Ty::Nil] };
            assert!(!never_falsy(&left), "{left:?} must not always short-circuit `||`");
            assert_eq!(falsy_part(&left), Some(Ty::Nil), "{left:?} must retain nil for `&&`");
        }
    }

    #[test]
    fn case_match_hash_pattern_binds_value_omission_key_from_hash_value_ty() {
        use crate::expr::{MatchArm, MatchPattern};

        // case h
        // in {status: "ok", data:}
        //   data
        // end
        //
        // `h` is statically `Hash[Sym, Str]`; the value-omission bind
        // `data:` should type as the hash's uniform value type (`Str`),
        // and so should the arm body that reads it back.
        let h_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) };
        let pattern = MatchPattern::Hash {
            constant: None,
            pairs: vec![
                (
                    Symbol::from("status"),
                    Some(MatchPattern::Value {
                        expr: synth(ExprNode::Lit {
                            value: Literal::Str { value: "ok".to_string() },
                        }),
                    }),
                ),
                (Symbol::from("data"), None),
            ],
            rest: None,
        };
        let arm = MatchArm { pattern, guard: None, body: var("data") };
        let mut expr = synth(ExprNode::CaseMatch {
            scrutinee: var("h"),
            arms: vec![arm],
            else_body: None,
        });

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = ctx_with_local("h", h_ty);
        let ty = typer.analyze_expr(&mut expr, &ctx);
        assert_eq!(ty, Ty::Str, "case/in result should be the arm body's type");

        let ExprNode::CaseMatch { arms, .. } = &*expr.node else {
            panic!("expected CaseMatch, got {:?}", expr.node);
        };
        assert_eq!(
            arms[0].body.ty,
            Some(Ty::Str),
            "`data` should read as the hash's value type, not Untyped"
        );
    }

    #[test]
    fn case_match_array_pattern_with_constant_binds_untyped() {
        use crate::expr::{MatchArm, MatchPattern};

        // case r
        // in Success(page)
        //   page
        // else
        //   nil
        // end
        //
        // `Success` is an unregistered (gem) class — a constant-
        // narrowed pattern's bindings type `Untyped`, a deliberate
        // gradual opt-out rather than an error (mirrors how a bare
        // `Const` read of an unknown class always resolves rather than
        // failing to type at all).
        let pattern = MatchPattern::Array {
            constant: Some(synth(ExprNode::Const { path: vec![Symbol::from("Success")] })),
            pre: vec![MatchPattern::Bind { name: Symbol::from("page") }],
            rest: None,
            post: vec![],
        };
        let arm = MatchArm { pattern, guard: None, body: var("page") };
        let mut expr = synth(ExprNode::CaseMatch {
            scrutinee: var("r"),
            arms: vec![arm],
            else_body: Some(nil_lit()),
        });

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = ctx_with_local("r", Ty::Untyped);
        typer.analyze_expr(&mut expr, &ctx);

        let ExprNode::CaseMatch { arms, .. } = &*expr.node else {
            panic!("expected CaseMatch, got {:?}", expr.node);
        };
        assert_eq!(arms[0].body.ty, Some(Ty::Untyped));
    }
}

/// The arms of `ty` a falsy value can come from -- `nil`, `false`, and
/// anything not known to be truthy. `None` when there are none.
fn falsy_part(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Union { variants } => {
            let kept: Vec<Ty> = variants.iter().filter_map(falsy_part).collect();
            match kept.len() {
                0 => None,
                1 => kept.into_iter().next(),
                _ => Some(Ty::Union { variants: kept }),
            }
        }
        t if never_falsy(t) => None,
        t => Some(t.clone()),
    }
}

/// Can this type NEVER be Ruby-falsy?
///
/// Ruby's falsy set is exactly `nil` and `false`; every other object,
/// `0` and `""` included, is truthy. So a type that admits neither
/// makes `x || y` short-circuit on every run, and the result is `x`'s
/// type rather than a union with `y`'s.
///
/// A whitelist, not a blacklist: `Untyped`, `Var` and anything else the
/// enum grows must fall through to the union, because being wrong here
/// drops a real arm from a real type.
fn never_falsy(ty: &Ty) -> bool {
    match ty {
        Ty::Int
        | Ty::Float
        | Ty::Str
        | Ty::Sym
        | Ty::Date
        | Ty::Time
        | Ty::Array { .. }
        | Ty::Hash { .. }
        | Ty::Tuple { .. }
        | Ty::Record { .. }
        | Ty::Relation { .. }
        | Ty::Class { .. }
        | Ty::Fn { .. } => true,
        // `Bool` is the whole point of the exclusion — it is the one
        // scalar that carries `false`.
        Ty::Union { variants } => variants.iter().all(never_falsy),
        _ => false,
    }
}

// Not the `(str, now)` form: only one argument is grounded to `ActiveSupport.parse_time` / `zone_parse`.
fn time_parse_ty(recv: &Expr, method: &Symbol, args: &[Expr]) -> Option<Ty> {
    if method.as_str() != "parse" || args.len() != 1 {
        return None;
    }
    if is_time_const(recv) {
        return Some(Ty::Time);
    }
    match &*recv.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "zone" && args.is_empty() && is_time_const(r) =>
        {
            Some(Ty::Union { variants: vec![Ty::Time, Ty::Nil] })
        }
        _ => None,
    }
}

fn is_time_const(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Time")
}

// Not the value type for `expect(article: [:title, …])`: it answers the permitted Parameters; the array forms (`ids: []`, `[[…]]`) stay unmodeled until their lowering runs.
fn expect_hash_arg_ty(recv_ty: Option<&Ty>, method: &str, args: &[crate::expr::Expr]) -> Option<Ty> {
    use crate::expr::ExprNode;
    if method != "expect" {
        return None;
    }
    let Some(Ty::Hash { key, value }) = recv_ty else { return None };
    let [arg] = args else { return None };
    let ExprNode::Hash { entries, .. } = &*arg.node else { return None };
    let [(_, permitted)] = entries.as_slice() else { return None };
    let ExprNode::Array { elements, .. } = &*permitted.node else { return None };
    if elements.is_empty() || !elements.iter().all(|e| matches!(&*e.node, ExprNode::Lit { value: crate::expr::Literal::Sym { .. } })) {
        return None;
    }
    Some(Ty::Hash { key: key.clone(), value: value.clone() })
}

// Not every `params[...]` read: a scalar read stays a String, and only a call no String answers (or a Symbol-keyed index) reads the request's nested shape.
fn promotes_to_param_value(
    recv: &crate::expr::Expr,
    recv_ty: Option<&Ty>,
    method: &crate::ident::Symbol,
    args: &[crate::expr::Expr],
    locals: &HashMap<Symbol, Ty>,
) -> bool {
    use crate::expr::{ExprNode, Literal};
    let stringish = match recv_ty {
        Some(Ty::Str) => true,
        Some(Ty::Union { variants }) => {
            variants.iter().any(|v| matches!(v, Ty::Str)) && variants.iter().all(|v| matches!(v, Ty::Str | Ty::Nil))
        }
        _ => false,
    };
    // Not only the source's `params`: after the controller lowering it reads `@params`, a `Hash[String, untyped]`.
    let untyped = match recv_ty {
        Some(Ty::Untyped) => true,
        Some(Ty::Union { variants }) => variants.iter().all(|v| matches!(v, Ty::Untyped | Ty::Nil)),
        _ => false,
    };
    let lowered = untyped && is_ivar_params_rooted(recv, locals);
    if !(stringish && is_params_rooted(recv, locals)) && !lowered {
        return false;
    }
    let keyed = matches!(
        method.as_str(),
        "[]" | "[]=" | "fetch" | "dig" | "key?" | "has_key?" | "delete" | "slice" | "except"
    ) && matches!(args.first().map(|a| &*a.node), Some(ExprNode::Lit { value: Literal::Sym { .. } }));
    let structural = STRUCTURAL.contains(&method.as_str());
    keyed || (structural && matches!(send::str_method(method), Ty::Var { .. }))
}

// Not a field on `Ctx`: a name no Ruby local can take rides in `local_bindings`, which every scope already clones.
fn param_local_mark(name: &Symbol) -> Symbol {
    Symbol::from(format!("#params-local:{}", name.as_str()).as_str())
}

fn is_param_local(e: &crate::expr::Expr, locals: &HashMap<Symbol, Ty>) -> bool {
    matches!(&*e.node, crate::expr::ExprNode::Var { name, .. } if locals.contains_key(&param_local_mark(name)))
}

fn reads_structurally(e: &crate::expr::Expr, name: &Symbol) -> bool {
    use crate::expr::{ExprNode, Literal};
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
        if matches!(&*r.node, ExprNode::Var { name: n, .. } if n == name) {
            let keyed = matches!(method.as_str(), "[]" | "[]=" | "fetch" | "dig" | "key?" | "has_key?")
                && matches!(args.first().map(|a| &*a.node), Some(ExprNode::Lit { value: Literal::Sym { .. } }));
            if keyed || STRUCTURAL.contains(&method.as_str()) {
                return true;
            }
        }
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found = found || reads_structurally(c, name));
    found
}

const STRUCTURAL: &[&str] = &[
    "each", "each_pair", "each_value", "each_key", "each_with_index", "reverse_each", "map", "collect",
    "flat_map", "filter_map", "select", "filter", "reject", "any?", "all?", "none?", "keys", "values", "to_a",
    "first", "last", "compact", "uniq", "sort", "sort_by", "merge", "key?", "has_key?", "dig", "permit",
    "permit!", "to_unsafe_h", "require",
];

fn is_params_rooted(e: &crate::expr::Expr, locals: &HashMap<Symbol, Ty>) -> bool {
    use crate::expr::ExprNode;
    match &*e.node {
        ExprNode::Send { recv: None, method, args, .. } => method.as_str() == "params" && args.is_empty(),
        ExprNode::Send { recv: Some(r), method, .. } => {
            matches!(method.as_str(), "[]" | "fetch" | "dig" | "require") && is_params_rooted(r, locals)
        }
        _ => is_param_local(e, locals),
    }
}

fn is_ivar_params_rooted(e: &crate::expr::Expr, locals: &HashMap<Symbol, Ty>) -> bool {
    use crate::expr::ExprNode;
    match &*e.node {
        ExprNode::Send { recv: Some(r), method, .. }
            if matches!(method.as_str(), "[]" | "fetch" | "dig" | "slice" | "except" | "to_h" | "merge") =>
        {
            matches!(&*r.node, ExprNode::Ivar { name } if name.as_str() == "params")
                || is_ivar_params_rooted(r, locals)
                || is_param_local(r, locals)
        }
        _ => is_param_local(e, locals),
    }
}
