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

use crate::det::HashMap;
use rubydex::model::identity_maps::IdentityHashMap;
use rubydex::model::ids::{DeclarationId, declaration_id_from_lookup_name};

use crate::expr::{Expr, ExprNode, HashRest, LValue, Literal, MatchPattern};
use crate::ident::{ClassId, Symbol, TyVar};
use crate::ty::{Row, Ty};

/// A break in a bytes block can replace the method's String result. Nested
/// lambdas/iterators and while/until loops own their breaks independently.
// `Class[C]` is a class object that dispatch unwraps to `C`.
fn instance_shaped(ty: &Ty) -> bool {
    match ty {
        Ty::Class { id, .. } => id.0.as_str() != "Class",
        Ty::Union { variants } => variants.iter().all(|v| matches!(v, Ty::Nil) || instance_shaped(v)),
        _ => false,
    }
}

fn singleton_shaped(ty: &Ty) -> bool {
    match ty {
        Ty::Union { variants } => !variants.is_empty() && variants.iter().all(singleton_shaped),
        _ => super::singleton_of(Some(ty)).is_some(),
    }
}

fn bytes_block_has_escaping_break(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Break { .. } => true,
        ExprNode::Lambda { .. } | ExprNode::While { .. } => false,
        _ => {
            let mut found = false;
            e.node.for_each_child(&mut |child| found |= bytes_block_has_escaping_break(child));
            found
        }
    }
}

/// A forwarded `&local` whose binding may be nil at runtime — Ruby then
/// passes no block. Bare `Nil` is handled separately as definite absence.
fn forwarded_block_may_be_nil(ty: &Ty) -> bool {
    matches!(ty, Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)))
}

mod diagnostic;
pub(crate) use diagnostic::some_operands_take_it;
mod const_resolution;
mod lambda_local;
pub(crate) use const_resolution::{ConstResolver, ConstResolverTask, definition_index, has_ambiguous_namespace};
pub use const_resolution::DefinitionIndex;
pub use const_resolution::PreparedConstResolver;
use const_resolution::ResolvedConstant;
mod narrowing;
mod pagy;
mod send;
mod active_storage;
pub(crate) use send::PARAM_VALUE;
pub(crate) use send::{indexed_pluck, string_answers};
#[cfg(test)]
pub(crate) use send::counted_first_last;

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

    /// Only the constants this scope's class declares itself.
    pub fn get_own(&self, name: &Symbol) -> Option<&Ty> {
        self.own.get(name)
    }

    /// Only the app-wide, by-bare-name registry.
    pub fn get_global(&self, name: &Symbol) -> Option<&Ty> {
        self.global.get(name)
    }
}

const RUNTIME_MIXIN: &str = "Module mixin at run time";

/// Recursion context — what `self` is, what locals/ivars are in scope.
/// Immutable during descent; clone to enter a new scope (Let body,
/// block body, Seq walk with new ivar/local bindings).
#[derive(Clone, Default)]
pub struct Ctx {
    pub self_ty: Option<Ty>,
    /// What `self_ty` was before a self binding (`T.bind(self, T)`,
    /// `#: self as T`) replaced it for the rest of a body. Names in a
    /// declared type still resolve in the lexical namespace, not in
    /// the one `self` was bound to. `None` while nothing is bound.
    pub lexical_self: Option<Ty>,
    /// Ivar bindings observed as a `Seq` walks its statements in order.
    /// `@post = Post.find(...)` in stmt 1 lets `@post.destroy` in stmt 2
    /// dispatch correctly.
    pub ivar_bindings: HashMap<Symbol, Ty>,
    /// Local-variable bindings in the current scope: let-bound names,
    /// assignments accumulated through a `Seq`, and block parameters
    /// seeded from a receiver-aware dispatch.
    pub local_bindings: HashMap<Symbol, Ty>,
    /// Locals whose assigned value is proven to be a class/module object.
    /// Nominal instance types alone do not establish this identity.
    pub class_objects: crate::det::HashSet<Symbol>,
    /// Locals an `is_a?(Class)` / `is_a?(Module)` guard proved class objects of unknown identity: Module queries only (`answers_module_queries`).
    pub query_class_objects: crate::det::HashSet<Symbol>,
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
    /// The template being typed (`articles/_form`), for a lazy `t(".key")`.
    pub view_name: Option<Symbol>,
    /// Set while typing the body of a class-side method (`def self.x`,
    /// `class << self; def x`). `self` there is the class object, so an
    /// implicit-self `new` answers "an instance of whichever class
    /// received the call", not of the class the `def` sits in.
    pub class_side: bool,
    /// Set while typing a class-method whose name is a first-class model
    /// DSL lowerer (`has_markdown`, `has_rich_text`, …). Those bodies are
    /// macro templates: association/`scope`/`class_eval` leftovers are
    /// claimed by the dedicated lowerer at call sites, so attaching
    /// `ActiveRecord::Base … lacks a shared runtime` on the template
    /// itself is noise that hides the real ledger.
    pub claimed_macro_template: bool,
    // `!class_side` cannot stand in: scope bodies type with it false, and their bare `active` is the scope.
    pub instance_body: bool,
    // `class_side` cannot stand in: a `class_eval` block in an initializer is class-side too, and its `include` runs at load
    pub method_body: bool,
    /// Implicit `self` is a class still being assembled: a Module hook's
    /// receiver, or a `Class.new`/`Module.new` block's new class.
    pub building_self: bool,
    /// Locals bound to a class the program already has (a constant, or
    /// `self` on the class side), not one `Class.new` is building.
    pub live_class_locals: crate::det::HashSet<Symbol>,
    /// Typing an `app/helpers` module's body. Its `self` may be the
    /// module's sole includer (a controller), but the emitted body runs
    /// in the view, where a bare `render` is ActionView's String.
    pub helper_body: bool,
}

/// User-class dispatch data: table name (if any), instance shape,
/// class/instance method tables. Built by [`super::Analyzer`] from
/// Rails schema + conventions; the body-typer reads it.
#[derive(Default, Clone)]
pub struct ClassInfo {
    /// Kind of the indexed source declaration; never inferred from its name.
    pub is_module: bool,
    /// `@@name` storage a `cattr_*`/`mattr_*` declares, typed by its default.
    pub class_vars: HashMap<Symbol, Ty>,
    /// Constant values declared by external gem RBI/RBS files.
    pub constants: HashMap<Symbol, Ty>,
    /// If this class maps to a database table, which one.
    pub table: Option<crate::ident::TableRef>,
    pub primary_key: Option<Symbol>,
    /// The model declares a `default_scope` the runtime Relation applies
    /// (`lower::default_scope`): a query on it must not be folded to SQL
    /// at transpile time, which would read the rows the scope hides.
    pub default_scoped: bool,
    /// Instance-state shape (columns + attr_accessor).
    pub attributes: Row,
    /// Attributes of `attributes` that the app's own `def` replaces. A
    /// `def confidence` computes whatever it likes, so two reads of it need
    /// not agree and a guard on one says nothing about the other.
    pub overridden_attributes: crate::det::HashSet<Symbol>,
    /// Methods callable on the class itself: `Post.all`, `Post.find(id)`.
    pub class_methods: HashMap<Symbol, Ty>,
    /// Catalog entries and source methods can have the same gradual
    /// return type, so a call-site refinement needs their provenance.
    pub source_class_methods: crate::det::HashSet<Symbol>,
    /// Instance methods a model, library class or controller defines in
    /// source, known before the first harvest registers their returns.
    pub source_instance_methods: crate::det::HashSet<Symbol>,
    /// Source declares its own class-side `new`, whose result need not be self.
    pub declares_constructor: bool,
    /// A source `new` that calls `super` still needs the inherited constructor.
    pub constructor_calls_super: bool,
    /// Source class methods that call `super`, reaching the inherited one.
    pub delegating_class_methods: crate::det::HashSet<Symbol>,
    /// Source supplies an initializer body without delegating to `super`.
    pub implements_initializer: bool,
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
    pub relation_derived: crate::det::HashSet<Symbol>,
    /// Scopes whose body the seed classifier read as ending in a
    /// MATERIALIZING terminal — `ordered.last(PAGE_SIZE)` — as opposed
    /// to the ones it could not read at all. Both kinds may carry the
    /// `Array[Self]` seed (the counted terminal by classification, the
    /// unreadable body as the conservative stand-in), and only this set
    /// tells them apart at a Relation receiver: a materializing scope
    /// answers its Array, an unclassified one is assumed to still be a
    /// query and re-wraps as the relation.
    pub materializing_scopes: crate::det::HashSet<Symbol>,
    /// Methods whose every return is the value of the BLOCK they were
    /// called with — `yield` on each path, or the block handed on to
    /// another such method (lobsters' `get_from_cache(opts, &)`, which
    /// answers `yield` uncached and `Rails.cache.fetch(key, &)` cached).
    /// No signature can say that without method generics, which the RBS
    /// reader does not model, so dispatch answers the CALL SITE's block
    /// type for these, the way it already does for `then` and
    /// `transaction`. Harvested each fixpoint round
    /// (`Analyzer::harvest_method_returns`).
    pub block_value_methods: crate::det::HashSet<Symbol>,
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
    /// has_many readers without `through:` — the ones `lower::scope_chain`
    /// roots onto a relation, so a relation terminal on them runs as SQL.
    pub direct_has_many: crate::det::HashSet<Symbol>,
    /// Modules mixed in via `include` (e.g. a controller's
    /// `include IntervalHelper`). A mixed-in module's instance methods
    /// become instance methods of the includer, so dispatch consults
    /// these (their own instance_methods in the registry) after the
    /// class's own methods and before walking the parent. Empty for
    /// most classes.
    pub includes: Vec<crate::ident::ClassId>,
    /// Declared by a gem's RBI rather than by the app or the catalog
    /// (`crate::gem_boundary`). Its own table is the gem's whole
    /// declared surface, so a miss on the class itself is a real miss --
    /// but an APP class that inherits from it also inherits whatever the
    /// gem's macros generate, which no declaration lists.
    pub gem_boundary: bool,
    /// The surface cannot be enumerated (`method_missing`, an unresolved
    /// mixin, an ancestor the RBI could not name): a lookup that misses
    /// is unknown, not wrong.
    pub open: bool,
    /// Declared in the app's own source (a model, controller, or
    /// library class), as opposed to the framework catalog. An app class
    /// is reachable from a bare name only through Ruby's constant
    /// lookup (the lexical scope, then the ancestors), never by its last
    /// segment alone.
    pub app_declared: bool,
    pub positional_struct: bool,
}

impl ClassInfo {
    pub(crate) fn source_class_method(
        classes: &HashMap<ClassId, ClassInfo>, model: &ClassId, method: &Symbol,
    ) -> bool {
        let mut current = Some(model);
        for _ in 0..32 {
            let Some(class) = current.and_then(|id| classes.get(id)) else { return false };
            if class.source_class_methods.contains(method) {
                return true;
            }
            current = class.parent.as_ref();
        }
        false
    }

    /// Whether `name` is one of this model's real SCHEMA TABLE columns.
    /// `attributes` is built once, straight off `Schema::tables` (see
    /// `ingest::model::row_from_table`), never merged with method-only
    /// surface like a `has_secure_password` reader or a plain `def` —
    /// so this is a strictly narrower, more precise test than "does
    /// `instance_methods` know this name," which also answers yes for
    /// synthesized non-column readers (`password_reset_token`). Shared
    /// by the body-typer's and the arel lowerer's dynamic-finder
    /// handling (#558) so a per-column check can't drift between them.
    pub fn has_schema_column(&self, name: &Symbol) -> bool {
        self.attributes.fields.contains_key(name)
    }
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
/// The class a written owner path names, resolved the way Ruby
/// resolves a constant: the enclosing scopes from the inside out, then
/// the top level.
///
/// `DEFAULT_MODE = Mode::Fill` inside `UI::Selector` means
/// `UI::Selector::Mode`, and nothing else does — which matters in an
/// app where a dozen components each declare their own `Mode`. Walking
/// the nesting is what tells them apart; a suffix match over the class
/// registry cannot, and gives up on the ambiguity instead.
/// A class name written inside `scope` (a qualified class name),
/// resolved lexically: `scope::name`, then each enclosing namespace,
/// and finally the name as written. `None` when no registered class
/// answers, so an unknown name (a gem's class) is left as written.
pub(crate) fn lexical_class<T>(
    written: &ClassId,
    scope: &str,
    classes: &HashMap<ClassId, T>,
) -> Option<ClassId> {
    let name = written.0.as_str();
    let mut parts: Vec<&str> = scope.split("::").filter(|s| !s.is_empty()).collect();
    while !parts.is_empty() {
        let candidate = ClassId(Symbol::from(format!("{}::{name}", parts.join("::")).as_str()));
        if classes.contains_key(&candidate) {
            return Some(candidate);
        }
        parts.pop();
    }
    classes.contains_key(written).then(|| written.clone())
}

/// Reusable body-type walker. Holds a borrow of the dispatch table so
/// repeated `analyze_expr` calls reuse the same lookup structures
/// without cloning.
pub struct BodyTyper<'a> {
    classes: &'a HashMap<ClassId, ClassInfo>,
    materialized_associations: Option<&'a crate::det::HashSet<Symbol>>,
    const_resolver: Option<std::sync::Arc<ConstResolver>>,
    typed_constants: Option<&'a IdentityHashMap<DeclarationId, Ty>>,
    data_factories: Option<&'a HashMap<crate::span::Span, Ty>>,
    class_factories: Option<&'a HashMap<crate::span::Span, Ty>>,
    /// Methods whose value is an ActiveSupport inquirer (see
    /// [`crate::analyze::inquiry`]); empty for the bare constructor,
    /// which the runtime-source typer and tests use.
    inquirers: Option<&'a crate::det::HashSet<Symbol>>,
    /// The app's translations; Rails' English when absent.
    i18n: Option<&'a crate::i18n::Catalog>,
    helper_modules: Option<&'a crate::det::HashSet<ClassId>>,
    /// The `app/helpers` module that answers each helper name in the view.
    helper_index: Option<&'a HashMap<Symbol, ClassId>>,
    open_classes: Option<&'a crate::analyze::open_classes::OpenClasses>,
    /// Parameter types for the lambda assignment currently being retyped
    /// from its `.call` sites. Empty during an ordinary walk.
    lambda_seeds: std::cell::RefCell<Option<lambda_local::Seeds>>,
    /// The block parameter `block_ctx_for` proved a class object (`klass.tap { |k| }`), for the Lambda arm that clears its parameters.
    block_class_param: std::cell::RefCell<Option<Symbol>>,
    /// The locals of the outermost expression being typed, counted once
    /// for the selector proofs of `active_storage`.
    scope_bindings: std::cell::RefCell<Option<active_storage::ScopeBindings>>,
    /// The block parameter a literal symbol list binds (`%i[a b].each { |m| }`), for the Lambda arm.
    block_selector_param: std::cell::RefCell<Option<(Symbol, Vec<Symbol>)>>,
}

impl<'a> BodyTyper<'a> {
    /// Submodule accessor for the dispatch table. `classes` itself
    /// stays private to this module; `send.rs` reaches it here.
    pub(super) fn classes(&self) -> &'a HashMap<ClassId, ClassInfo> {
        self.classes
    }

    fn answers_receiver(&self, ty: &Ty, method: &Symbol) -> bool {
        match ty {
            Ty::Union { variants } => variants.iter().all(|v| self.answers_receiver(v, method)),
            Ty::Class { id, .. } => {
                let mut current = Some(id);
                for _ in 0..32 {
                    let Some(cid) = current else { return true };
                    let Some(cls) = self.classes.get(cid) else { return false };
                    if cls.open || cls.instance_methods.contains_key(method) {
                        return false;
                    }
                    current = cls.parent.as_ref();
                }
                false
            }
            Ty::Relation { .. } | Ty::Array { .. } | Ty::Hash { .. } | Ty::Tuple { .. } | Ty::Str | Ty::Int => true,
            _ => false,
        }
    }

    fn has_unknown_ancestor(&self, id: &crate::ident::ClassId) -> bool {
        let mut current = Some(id);
        for step in 0..32 {
            let Some(cid) = current else { return false };
            let Some(cls) = self.classes.get(cid) else { return step > 0 };
            if cls.open {
                return true;
            }
            current = cls.parent.as_ref();
        }
        false
    }

    /// Whether `self`'s class, its includes or its ancestors register
    /// `method` — an app definition, whatever type it answered.
    /// [`Self::app_defines`] for one side: an instance body is shadowed by
    /// instance methods (own, included, inherited), a class-method body by
    /// class-side ones only, since a class object cannot dispatch an
    /// instance method.
    fn app_defines_on_side(&self, self_ty: Option<&Ty>, method: &Symbol, instance: bool) -> bool {
        let Some(Ty::Class { id, .. }) = self_ty else { return false };
        let mut stack = vec![id];
        let mut seen = crate::det::HashSet::default();
        while let Some(cid) = stack.pop() {
            if !seen.insert(cid) {
                continue;
            }
            let Some(cls) = self.classes.get(cid) else { continue };
            let table = if instance { &cls.instance_methods } else { &cls.class_methods };
            if table.contains_key(method) {
                return true;
            }
            if instance {
                stack.extend(cls.includes.iter());
            }
            stack.extend(cls.parent.iter());
        }
        false
    }

    /// The lazy-lookup scope of a controller whose instance methods have
    /// `AbstractController::Translation`'s `t`: one descending from
    /// `ActionController::Base` that defines no `t` / `translate` of its
    /// own (see `lower::i18n_translate`).
    fn translating_controller(&self, self_ty: Option<&Ty>) -> Option<String> {
        let Some(Ty::Class { id, .. }) = self_ty else { return None };
        if ["t", "translate"].iter().any(|m| self.app_defines(self_ty, &Symbol::from(*m))) {
            return None;
        }
        let mut current = id;
        for _ in 0..32 {
            let parent = self.classes.get(current)?.parent.as_ref()?;
            if parent.0.as_str() == "ActionController::Base" {
                return Some(crate::lower::i18n_translate::controller_scope(id.0.as_str()));
            }
            current = parent;
        }
        None
    }

    /// Whether `recv` is a model class, whose setter is `ActiveRecord::Base`'s.
    fn is_model_class(&self, recv: Option<&Ty>) -> bool {
        let Some(Ty::Class { id, .. }) = recv else { return false };
        let mut current = id;
        for _ in 0..32 {
            // Not `ActiveRecord::Base` alone: the registry leaves `ApplicationRecord`'s parent unset.
            if matches!(current.0.as_str(), "ActiveRecord::Base" | "ApplicationRecord") {
                return true;
            }
            let Some(parent) = self.classes.get(current).and_then(|c| c.parent.as_ref()) else { return false };
            current = parent;
        }
        false
    }

    fn app_defines(&self, self_ty: Option<&Ty>, method: &Symbol) -> bool {
        let Some(Ty::Class { id, .. }) = self_ty else { return false };
        let mut stack = vec![id];
        let mut seen = crate::det::HashSet::default();
        while let Some(cid) = stack.pop() {
            if !seen.insert(cid) {
                continue;
            }
            let Some(cls) = self.classes.get(cid) else { continue };
            if cls.instance_methods.contains_key(method) || cls.class_methods.contains_key(method) {
                return true;
            }
            stack.extend(cls.includes.iter());
            stack.extend(cls.parent.iter());
        }
        false
    }

    pub fn new(classes: &'a HashMap<ClassId, ClassInfo>) -> Self {
        Self {
            classes,
            materialized_associations: None,
            const_resolver: None,
            typed_constants: None,
            data_factories: None,
            class_factories: None,
            inquirers: None,
            i18n: None,
            helper_modules: None,
            helper_index: None,
            open_classes: None,
            lambda_seeds: std::cell::RefCell::new(None),
            block_class_param: std::cell::RefCell::new(None),
            scope_bindings: std::cell::RefCell::new(None),
            block_selector_param: std::cell::RefCell::new(None),
        }
    }

    /// Retype lambdas stored in this sequence once a later statement has
    /// called them. Evidence is re-read from the statement and from every
    /// stored lambda after each retype, so `run = ->(x) { add.call(x) }`
    /// seeds `add` once `x` is known. The call statement is retyped once
    /// at the end so its result sees the new returns.
    fn refine_lambda_locals(
        &self,
        exprs: &mut [Expr],
        stmt: usize,
        local_ctx: &mut Ctx,
        lambdas: &mut HashMap<Symbol, lambda_local::Stored>,
        last: &mut Ty,
    ) {
        let mut stmt_dirty = false;
        // Mutual recursion can wrap a parameter in a deeper array every
        // round (`f` calls `g` calls `f`). `merge` only unions, and that
        // union does not stop the nesting, so the loop needs a fuel cap.
        let mut rounds = lambdas.len().saturating_mul(4).saturating_add(4);
        loop {
            if rounds == 0 {
                break;
            }
            rounds -= 1;
            let names: Vec<Symbol> = lambdas.keys().cloned().collect();
            let mut seeded = Vec::new();
            for name in &names {
                let evidence = lambda_call_evidence(exprs, stmt, lambdas, name);
                let Some(stored) = lambdas.get_mut(name) else { continue };
                if lambda_local::merge(stored, &evidence) {
                    seeded.push(name.clone());
                }
            }
            if seeded.is_empty() {
                break;
            }
            stmt_dirty = true;
            self.retype_seeded_lambdas(exprs, local_ctx, lambdas, seeded);
        }
        if !stmt_dirty {
            return;
        }
        // The statement is the lambda assignment itself. It was retyped
        // with its seeds above; analyzing it again would type the lambda
        // with none.
        if lambdas.values().any(|stored| stored.index == stmt) {
            *last = exprs[stmt].ty.clone().unwrap_or_else(unknown);
            return;
        }
        *last = self.analyze_expr(&mut exprs[stmt], local_ctx);
    }

    /// Retype each seeded lambda, then any lambda whose body calls one
    /// whose type just changed. `merge` only unions, and a retype is
    /// skipped once the type stops moving, so the fuel cap is a backstop.
    fn retype_seeded_lambdas(
        &self,
        exprs: &mut [Expr],
        local_ctx: &mut Ctx,
        lambdas: &HashMap<Symbol, lambda_local::Stored>,
        seeded: Vec<Symbol>,
    ) {
        let mut pending = seeded;
        let mut fuel = lambdas.len().saturating_mul(lambdas.len()).saturating_add(1);
        while let Some(name) = pending.pop() {
            if fuel == 0 {
                break;
            }
            fuel -= 1;
            if !self.retype_stored_lambda(exprs, local_ctx, lambdas, &name) {
                continue;
            }
            for (other, stored) in lambdas.iter() {
                if other == &name || pending.contains(other) {
                    continue;
                }
                if !lambda_local::calls_in(&exprs[stored.index], &name).is_empty() {
                    pending.push(other.clone());
                }
            }
        }
    }

    /// Retype one stored lambda. False when its type did not change.
    fn retype_stored_lambda(
        &self,
        exprs: &mut [Expr],
        local_ctx: &mut Ctx,
        lambdas: &HashMap<Symbol, lambda_local::Stored>,
        name: &Symbol,
    ) -> bool {
        let Some(stored) = lambdas.get(name) else { return false };
        let index = stored.index;
        let mut ctx = stored.ctx.clone();
        let seeds = lambda_local::seeds_of(stored);
        for (other, ty) in &local_ctx.local_bindings {
            if lambdas.contains_key(other) {
                ctx.local_bindings.insert(other.clone(), ty.clone());
            }
        }
        *self.lambda_seeds.borrow_mut() = Some(seeds);
        let ty = self.analyze_expr(&mut exprs[index], &ctx);
        *self.lambda_seeds.borrow_mut() = None;
        let changed = local_ctx.local_bindings.get(name) != Some(&ty);
        local_ctx.local_bindings.insert(name.clone(), ty);
        changed
    }

    pub(crate) fn with_materialized_associations(mut self, names: &'a crate::det::HashSet<Symbol>) -> Self {
        self.materialized_associations = Some(names);
        self
    }

    pub(crate) fn with_open_classes(mut self, open: &'a crate::analyze::open_classes::OpenClasses) -> Self {
        self.open_classes = Some(open);
        self
    }

    pub(crate) fn with_i18n(mut self, catalog: &'a crate::i18n::Catalog, helper_modules: &'a crate::det::HashSet<ClassId>) -> Self {
        self.i18n = Some(catalog);
        self.helper_modules = Some(helper_modules);
        self
    }

    pub(crate) fn with_helper_index(mut self, index: &'a HashMap<Symbol, ClassId>) -> Self {
        self.helper_index = Some(index);
        self
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

    pub(super) fn with_class_factories(mut self, factories: &'a HashMap<crate::span::Span, Ty>) -> Self {
        self.class_factories = Some(factories);
        self
    }

    /// Name the app's inquirer-returning methods, so their predicates
    /// type as Bool instead of failing dispatch on `Str`.
    pub fn with_inquirers(
        mut self,
        inquirers: &'a crate::det::HashSet<Symbol>,
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
        // The outermost call types a whole body: count the locals it binds.
        let root = self.inquirers.is_some() && self.scope_bindings.borrow().is_none();
        if root {
            *self.scope_bindings.borrow_mut() = Some(active_storage::ScopeBindings::of(expr));
        }
        let ty = self.compute(expr, ctx);
        if root {
            *self.scope_bindings.borrow_mut() = None;
        }
        // Not overwritten by a re-analysis that knows less: the i18n lowering stamps the `I18n` runtime calls it writes, which the app registry does not carry. Only those: elsewhere the re-analysis is the newer fact
        let ty = match (&ty, &expr.ty, &*expr.node) {
            (fresh, Some(stamped), ExprNode::Send { recv: Some(r), .. })
                if matches!(fresh, Ty::Untyped | Ty::Var { .. })
                    && !stamped.is_open()
                    && matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "I18n") =>
            {
                stamped.clone()
            }
            _ => ty,
        };
        expr.ty = Some(ty.clone());
        expr.decisions &= !crate::expr::CLASS_OBJECT_VALUE;
        if self.is_class_object(expr, ctx) {
            expr.decisions |= crate::expr::CLASS_OBJECT_VALUE;
            let runtime_class = matches!(&*expr.node, ExprNode::Send { recv, method, .. } if method.as_str() == "singleton_class"
                    && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)))
                || (matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "new")
                    && !self.class_factories.is_some_and(|factories| factories.contains_key(&expr.span)));
            if runtime_class {
                expr.decisions |= crate::expr::RUNTIME_CLASS_OBJECT;
            }
        }
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
                        _ => Ty::Hash { key: std::sync::Arc::new(Ty::Untyped), value: std::sync::Arc::new(Ty::Untyped) },
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
                ctx.class_objects.remove(&name);
                ctx.query_class_objects.remove(&name);
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

    /// The type of `name` when it is an attribute of the class `self` is an
    /// instance of: a column or an `attr_accessor`/`attr_reader`, its own or an
    /// ancestor's. Such a read is a plain state read, which is what lets a
    /// guard on it (`expires_at.present? && expires_at > now`) speak for
    /// the reads that follow it the way a guard on a local does.
    ///
    /// The type is the reader's, not the column's: `serialize :metadata`,
    /// an enum or a money column registers a reader that answers something
    /// other than the storage type, and `return if metadata.nil?` must not
    /// narrow the reads after it back to the column's `String`.
    fn self_attribute_ty(&self, name: &Symbol, ctx: &Ctx) -> Option<Ty> {
        let Some(Ty::Class { id, .. }) = &ctx.self_ty else {
            return None;
        };
        let mut cursor = Some(id.clone());
        // Bounded: a parent link that cycles must not hang the typer.
        for _ in 0..16 {
            let Some(cur) = cursor else { break };
            let Some(info) = self.classes.get(&cur) else { break };
            if let Some(ty) = info.attributes.fields.get(name) {
                return Some(info.instance_methods.get(name).unwrap_or(ty).clone());
            }
            cursor = info.parent.clone();
        }
        None
    }

    /// The type of `local.attr` when that read is a stable attribute read:
    /// `local` is bound to a record class, and `attr` is a schema column (or
    /// `attr_accessor`) of it or of an ancestor, with no `def attr` of the
    /// app's own along the way. Such a read is a plain state read, so two
    /// reads with no write between them see the same value and a guard on
    /// one speaks for the other. A user-defined `attr` may compute anything,
    /// so it never qualifies.
    fn member_attribute_ty(&self, local: &Symbol, attr: &Symbol, ctx: &Ctx) -> Option<Ty> {
        let Some(Ty::Class { id, .. }) = ctx.local_bindings.get(local) else {
            return None;
        };
        let mut cursor = Some(id.clone());
        // Bounded: a parent link that cycles must not hang the typer.
        for _ in 0..16 {
            let cur = cursor?;
            let info = self.classes.get(&cur)?;
            if info.overridden_attributes.contains(attr) {
                return None;
            }
            if let Some(field) = info.attributes.fields.get(attr) {
                let ty = info.instance_methods.get(attr).unwrap_or(field);
                return Some(ty.clone());
            }
            cursor = info.parent.clone();
        }
        None
    }

    fn is_live_class(&self, expr: &Expr, ctx: &Ctx) -> bool {
        match &*expr.node {
            ExprNode::SelfRef => ctx.class_side && !ctx.building_self,
            ExprNode::Const { .. } => self.is_class_object(expr, ctx),
            ExprNode::Var { name, .. } => ctx.class_objects.contains(name) && ctx.live_class_locals.contains(name),
            _ => false,
        }
    }

    fn class_factory_receiver(&self, receiver: &Expr, method: &Symbol, ctx: &Ctx) -> bool {
        if method.as_str() != "new" { return false; }
        let ExprNode::Const { path } = &*receiver.node else { return false };
        let names: Vec<_> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
        let [name @ ("Class" | "Module")] = names.as_slice() else { return false };
        self.is_class_object(receiver, ctx)
            && self.const_resolver.as_ref().is_none_or(|r| r.is_runtime_class(receiver.span, path, name))
            && !self.owns_operator(receiver.ty.as_ref(), method, true)
            && !self.owns_operator(receiver.ty.as_ref(), &Symbol::from("initialize"), false)
    }

    fn has_singleton_class(&self, ty: Option<&Ty>, class_object: bool, method: &Symbol) -> bool {
        let instance = matches!(ty, Some(Ty::Class { id, .. })
            if self.classes().contains_key(id)
                && !matches!(id.0.as_str(), "Integer" | "Float" | "Symbol" | "Rational" | "Complex" | "NilClass" | "TrueClass" | "FalseClass")
                && self.has_kernel(id));
        (class_object || instance) && !self.owns_operator(ty, method, class_object)
    }

    // BasicObject instances do not answer Kernel#singleton_class unless
    // their ancestry includes Kernel. An unknown parent proves neither.
    fn has_kernel(&self, id: &ClassId) -> bool {
        let mut current = Some(id);
        for _ in 0..32 {
            let Some(id) = current else { return true };
            if id.0.as_str() == "Object" { return true; }
            let Some(class) = self.classes.get(id) else { return false };
            if class.includes.iter().any(|id| id.0.as_str() == "Kernel") { return true; }
            if id.0.as_str() == "BasicObject" { return false; }
            current = class.parent.as_ref();
        }
        false
    }

    /// Identity evidence for Module protocol, independent of nominal instance type.
    fn is_class_object(&self, expr: &Expr, ctx: &Ctx) -> bool {
        if self.class_factories.is_some_and(|factories| factories.contains_key(&expr.span)) {
            return true;
        }
        match &*expr.node {
            ExprNode::SelfRef => ctx.class_side,
            ExprNode::Var { name, .. } => ctx.class_objects.contains(name),
            ExprNode::Const { path } => {
                if expr.decisions & crate::expr::RESOLVED_CLASS_REF != 0 { return true; }
                if let Some(answer) = self.const_resolver.as_ref().and_then(|r| r.reference(expr.span, path)) {
                    if let Some(resolved) = answer {
                        return matches!(resolved, ResolvedConstant::Namespace { .. });
                    }
                }
                // An exact modeled namespace is valid without a source document.
                // A typed VALUE constant, even with the same nominal type, is not.
                let id = written_class_id(path);
                let value = if let [name] = path.as_slice() {
                    ctx.constants.get_own(name).or_else(|| ctx.constants.get_global(name))
                } else { None };
                if value.is_some() {
                    return false;
                }
                // Lowerers retype with a partial registry (no stdlib seed).
                // A Const whose computed type is the written class itself is
                // still the class object — `RecordNotFound < StandardError`
                // must stay Module#<, not Incompatible value comparison.
                self.classes.contains_key(&id)
                    || matches!(
                        expr.ty.as_ref(),
                        Some(Ty::Class { id: ty_id, .. }) if *ty_id == id
                    )
            }
            // Kernel#singleton_class answers the receiver's own singleton
            // class, one class object unique to that receiver: no subclass
            // can stand in, unlike `x.class` on a nominal type. Only for a
            // proven class/module object or an instance of a known
            // non-immediate class (`1.singleton_class` raises TypeError),
            // and not where an app method overrides it.
            ExprNode::Send { recv: Some(r), method, args, block: None, .. }
                if method.as_str() == "singleton_class" && args.is_empty() =>
            {
                self.has_singleton_class(r.ty.as_ref(), self.is_class_object(r, ctx), method)
            }
            ExprNode::Send { recv: None, method, args, block: None, .. }
                if method.as_str() == "singleton_class" && args.is_empty() =>
            {
                self.has_singleton_class(ctx.self_ty.as_ref(), ctx.class_side, method)
            }
            // `Module.new` / `Class.new(Parent)` answer a fresh module or
            // class, unless the app overrides `new` on them.
            ExprNode::Send { recv: Some(r), method, .. } => self.class_factory_receiver(r, method, ctx),
            _ => false,
        }
    }

    fn each_branch_is_class_object(&self, expr: &Expr, ctx: &Ctx) -> bool {
        match &*expr.node {
            ExprNode::If { then_branch, else_branch, .. } => {
                self.each_branch_is_class_object(then_branch, ctx) && self.each_branch_is_class_object(else_branch, ctx)
            }
            // A local that already answers Module queries passes that on, not an identity.
            ExprNode::Var { name, .. } if ctx.query_class_objects.contains(name) => true,
            _ => self.is_class_object(expr, ctx),
        }
    }

    /// A class object of unknown identity: any class answers a Module query
    /// as Ruby runs it, but a write through it (`define_method`) would land
    /// on whichever subclass it is, which no method table here can follow.
    fn answers_module_queries(&self, expr: &Expr, ty: Option<&Ty>, ctx: &Ctx) -> bool {
        if self.is_class_object(expr, ctx) || ty.is_some_and(singleton_shaped) {
            return true;
        }
        match &*expr.node {
            ExprNode::Send { recv: Some(r), method, args, block: None, .. } if args.is_empty() => match method.as_str() {
                "class" => !self.owns_operator(r.ty.as_ref(), method, self.is_class_object(r, ctx)),
                // `safe_constantize` is left out: it answers nil for a name nothing defines.
                "constantize" => matches!(r.ty.as_ref(), Some(Ty::Str))
                    && !self.owns_operator(Some(&Ty::Class { id: ClassId(Symbol::from("String")), args: vec![].into() }), method, false),
                _ => false,
            },
            ExprNode::Var { name, .. } => ctx.query_class_objects.contains(name)
                || matches!(ty, Some(Ty::Class { id, .. }) if matches!(id.0.as_str(), "Class" | "Module")),
            _ => false,
        }
    }

    // `x.class` is not a class reference to `is_class_object`, yet types as the same flat `Ty::Class` as an instance.
    fn is_instance(&self, expr: &Expr, ctx: &Ctx) -> bool {
        !self.is_class_object(expr, ctx)
            && !matches!(&*expr.node, ExprNode::Send { method, args, .. } if method.as_str() == "class" && args.is_empty())
    }

    /// An instance method of a class that descends from `Module`
    /// (`class Recorder < Module`) runs with a module as self: every
    /// instance of such a class is a module, whichever subclass.
    fn self_is_module_instance(&self, ctx: &Ctx) -> bool {
        if ctx.class_side { return false; }
        let Some(Ty::Class { id, .. }) = ctx.self_ty.as_ref() else { return false };
        let mut seen = crate::det::HashSet::default();
        let mut current = self.classes().get(id).and_then(|c| c.parent.clone());
        while let Some(id) = current {
            if matches!(id.0.as_str(), "Module" | "Class") { return true; }
            if !seen.insert(id.clone()) { break; }
            current = self.classes().get(&id).and_then(|c| c.parent.clone());
        }
        false
    }

    // An app class over Hash/Array/String/Set answers `include?` as membership, not Module#include?.
    fn descends_from_collection(&self, id: &ClassId) -> bool {
        let mut seen = crate::det::HashSet::default();
        let mut current = self.classes().get(id).and_then(|c| c.parent.clone());
        while let Some(id) = current {
            if matches!(id.0.as_str(), "Hash" | "Array" | "String" | "Set") { return true; }
            if !seen.insert(id.clone()) { break; }
            current = self.classes().get(&id).and_then(|c| c.parent.clone());
        }
        false
    }

    fn is_module_callback(&self, recv_ty: Option<&Ty>, method: &Symbol) -> bool {
        matches!(method.as_str(), "included" | "prepended" | "append_features" | "prepend_features")
            && matches!(recv_ty, Some(Ty::Class { id, .. }) if self.classes.get(id).is_some_and(|c| c.is_module && c.class_methods.contains_key(method)))
    }

    fn owns_operator(&self, ty: Option<&Ty>, method: &Symbol, class_object: bool) -> bool {
        match ty {
            Some(Ty::Union { variants }) => !variants.is_empty()
                && variants.iter().all(|ty| self.owns_operator(Some(ty), method, class_object)),
            Some(Ty::Class { id, .. }) => {
                let mut current = Some(id);
                for _ in 0..32 {
                    let Some(id) = current else { break };
                    let Some(class) = self.classes.get(id) else { break };
                    let own = if class_object { &class.class_methods } else { &class.instance_methods };
                    if own.contains_key(method) { return true; }
                    if !class_object && class.includes.iter().any(|m| self.lookup_in_module(m, method).is_some()) { return true; }
                    current = class.parent.as_ref();
                }
                false
            }
            _ => false,
        }
    }

    fn compute(&self, expr: &mut Expr, ctx: &Ctx) -> Ty {
        let expr_span = expr.span;
        match &mut *expr.node {
            ExprNode::Lit { value } => lit_ty(value),

            ExprNode::Const { path } => {
                // A later typing pass may resolve a value constant whose
                // owner was unknown in an earlier constant fixpoint round.
                crate::expr::ExprCold::set_diagnostic(&mut expr.cold, None);
                let generated = expr_span.file.0 == 0
                    || expr.decisions & crate::expr::GENERATED_CONST_REF != 0;
                // A real FileId outside the snapshot is an indexing
                // defect, not a generated expression or an ERB fallback.
                if !generated && self.const_resolver.as_ref()
                    .is_some_and(|resolver| !resolver.has_registered_source(expr_span.file))
                {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("constant"),
                        detail: format!("{}: missing source document for FileId({})",
                            written_class_id(path).0, expr_span.file.0),
                    }));
                    return unknown();
                }
                // Never search by suffix or borrow another scope's
                // same-named declaration: only the exact written class.
                let exact_modeled_class = |path: &[Symbol]| {
                    let id = match path {
                        [name] | [_, name] if name.as_str() == "HashWithIndifferentAccess" && (path.len() == 1 || path[0].as_str().is_empty()) => {
                            crate::ident::ClassId(Symbol::from(crate::analyze::indifferent::CLASS))
                        }
                        _ => written_class_id(path),
                    };
                    if let Some((name, owner)) = path.split_last() {
                        if !owner.is_empty() {
                            if let Some(info) = self.classes().get(&written_class_id(owner)) {
                                if info.gem_boundary {
                                    if let Some(ty) = info.constants.get(name) { return ty.clone(); }
                                }
                            }
                        }
                    }
                    if self.classes().contains_key(&id) {
                        Ty::Class { id, args: vec![].into() }
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
                    .filter(|_| !generated)
                    .and_then(|resolver| resolver.reference(expr_span, path));
                let ty = match source_reference {
                    Some(Some(ResolvedConstant::Namespace { class, runtime })) => {
                        let id = class.as_ref().clone();
                        if self.classes().contains_key(&id) || *runtime {
                            expr.decisions |= crate::expr::RESOLVED_CLASS_REF;
                            qualify_resolved_path(path, &id);
                            Ty::Class { id, args: vec![].into() }
                        } else {
                            unknown()
                        }
                    }
                    Some(Some(ResolvedConstant::Value { declaration, name, runtime, runtime_decl, singleton })) => {
                        // Concern splices retain the resolved value owner.
                        // Test-helper splice lifts a foreign module's
                        // constant onto the test class as a bare name
                        // (`DnsTestHelper::WEB_PUSH_PUBLIC_TEST_IP` →
                        // `WEB_PUSH_PUBLIC_TEST_IP`) and never emits the
                        // helper module — re-qualifying that bare path is a
                        // NameError at load. Leave bare paths bare when the
                        // class owns the leaf and the declaration's owner
                        // is foreign. Do not strip an already-qualified
                        // path (`FactoryModel::Result`) just because the
                        // leaf name also exists locally — that collision
                        // is a different constant (`data_factory_constants`).
                        let leaf = path.last();
                        let locally_owned = leaf
                            .is_some_and(|n| ctx.constants.get_own(n).is_some());
                        let enclosing = match &ctx.self_ty {
                            Some(Ty::Class { id, .. }) => Some(id.0.as_str()),
                            _ => None,
                        };
                        let decl_owner = name.0.as_str().rsplit_once("::").map(|(o, _)| o);
                        let keep_bare_splice = path.len() == 1
                            && locally_owned
                            && enclosing
                                .zip(decl_owner)
                                .is_some_and(|(enc, owner)| enc != owner);
                        if !keep_bare_splice {
                            qualify_resolved_path(path, name);
                            if *singleton {
                                expr.decisions |= crate::expr::SINGLETON_CONSTANT_REF;
                            }
                        }
                        let value = self.typed_constants
                            .and_then(|values| values.get(declaration))
                            .cloned()
                            .or_else(|| runtime.as_ref().map(|ty| (**ty).clone()));
                        let id: crate::ident::ClassId = (**name).clone();
                        match value {
                            Some(ty) => ty,
                            // Not left unknown: ingest turned `Result = Struct.new(…)` into the class it defines.
                            None if self.classes().get(&id).is_some_and(|c| c.app_declared) => {
                                expr.decisions |= crate::expr::RESOLVED_CLASS_REF;
                                Ty::Class { id, args: vec![].into() }
                            }
                            // A runtime façade's alias (`OpenSSL::Random =
                            // RandomSource`) of a class the registry models
                            // by that exact name: the registry's answer.
                            None if *runtime_decl && self.classes().contains_key(&id) => {
                                Ty::Class { id, args: vec![].into() }
                            }
                            None => unknown(),
                        }
                    }
                    // An unresolved source reference may still name an
                    // exact modeled external class (for example Time).
                    Some(None) => exact_modeled_class(path),
                    // A borrowed span is not evidence of a written constant
                    // only when the producer marked its generated origin.
                    None if indexed_source
                        && (generated || expr.decisions & crate::expr::UNINDEXED_SCOPE_CONST_REF != 0) =>
                    {
                        exact_modeled_class(path)
                    }
                    None if indexed_source => unknown(),
                    // Views and generated IR have no Rubydex answer. A
                    // bare name can use the app's bare-name values. A
                    // qualified name `A::B` names its owner, so only the
                    // value declared at that full name answers it.
                    None => {
                        let value = if let [name] = path.as_slice() {
                            ctx.constants.get_own(name).cloned()
                                .or_else(|| {
                                    let ty = exact_modeled_class(path);
                                    (!matches!(ty, Ty::Var { .. })).then_some(ty)
                                })
                                .or_else(|| ctx.constants.get_global(name).cloned())
                        } else {
                            let name = written_class_id(path);
                            self.typed_constants
                                .and_then(|values| {
                                    values.get(&declaration_id_from_lookup_name(name.0.as_str()))
                                })
                                .cloned()
                                // `Widget::WEIGHTS` read inside `Widget` (a
                                // body the analyzer already qualified, re-typed
                                // after lowering): the class's own constant.
                                .or_else(|| {
                                    let (leaf, owner) = path.split_last()?;
                                    let Some(Ty::Class { id, .. }) = ctx.lexical_self.as_ref().or(ctx.self_ty.as_ref()) else {
                                        return None;
                                    };
                                    if written_class_id(owner) != *id {
                                        return None;
                                    }
                                    ctx.constants.get_own(leaf).cloned()
                                })
                        };
                        // Otherwise retain the written class path, but
                        // never guess another class by its suffix.
                        value.unwrap_or_else(|| Ty::Class { id: written_class_id(path), args: vec![].into() })
                    }
                };
                // Not a class the emitted program has: the indifferent hash is a plain Hash there, so only its `new` is supported.
                if matches!(&ty, Ty::Class { id, args } if args.is_empty() && id.0.as_str() == crate::analyze::indifferent::CLASS) {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("constant"),
                        detail: written_class_id(path).0.as_str().to_string(),
                    }));
                }
                if indexed_source && matches!(ty, Ty::Var { .. }) {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("constant"),
                        detail: written_class_id(path).0.as_str().to_string(),
                    }));
                }
                ty
            }

            ExprNode::Var { name, .. } => ctx
                .local_bindings
                .get(name)
                .cloned()
                .or_else(|| {
                    let cvar = name.as_str().strip_prefix("@@")?;
                    let id = match ctx.self_ty.as_ref()? {
                        Ty::Class { id, args } if id.0.as_str() == "Class" && args.len() == 1 => match &args[0] {
                            Ty::Class { id, .. } => id,
                            _ => return None,
                        },
                        Ty::Class { id, .. } => id,
                        _ => return None,
                    };

                    self.classes().get(id)?.class_vars.get(&Symbol::from(cvar)).cloned()
                })
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
                        // `rescue A, B => e` binds the union: `e` is one of them. A class
                        // that failed constant resolution has already produced a
                        // blocking diagnostic. Its binding is unknown, rather than
                        // StandardError: inventing that class causes unrelated method
                        // errors and hides the original resolution failure in cascades.
                        let mut rescued: Option<Ty> = None;
                        for c in rc.classes.iter() {
                            match &c.ty {
                                Some(ty @ Ty::Class { .. }) => {
                                    rescued = Some(match rescued.take() {
                                        Some(prev) => union_of(prev, ty.clone()),
                                        None => ty.clone(),
                                    });
                                }
                                _ => {
                                    rescued = Some(Ty::Untyped);
                                    break;
                                }
                            }
                        }
                        let mut inner = ctx.clone();
                        inner.class_objects.remove(name);
                        inner.query_class_objects.remove(name);
                        inner.local_bindings.insert(
                            name.clone(),
                            rescued.unwrap_or_else(|| Ty::Class {
                                id: crate::ident::ClassId(Symbol::from("StandardError")),
                                args: vec![].into(),
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
                    key: std::sync::Arc::new(key_ty.unwrap_or_else(unknown)),
                    value: std::sync::Arc::new(value_ty.unwrap_or_else(unknown)),
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
                    // `[*a, b]` holds `a`'s elements, not `a`.
                    let et = if matches!(&*e.node, ExprNode::Splat { .. }) {
                        match splat_elements(et) {
                            Some(et) => et,
                            None => continue,
                        }
                    } else {
                        et
                    };
                    elem_ty = Some(match elem_ty.take() {
                        Some(prev) => union_of(prev, et),
                        None => et,
                    });
                }
                Ty::Array { elem: std::sync::Arc::new(elem_ty.unwrap_or_else(unknown)) }
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
                let reader = |n: &Symbol| self.self_attribute_ty(n, ctx);
                let member = |l: &Symbol, a: &Symbol| self.member_attribute_ty(l, a, &seeded);
                let pred = narrowing::extract_narrowing_with(left, &narrowing::Probe { reader: &reader, member: &member });
                let mut right_ctx = match (&pred, &*op) {
                    (Some(p), crate::expr::BoolOpKind::And) => {
                        narrowing::apply_narrowing(&seeded, p, true)
                    }
                    (Some(p), crate::expr::BoolOpKind::Or) => {
                        narrowing::apply_narrowing(&seeded, p, false)
                    }
                    _ => seeded.clone(),
                };
                // A narrowed `x.attr` stays valid only while `x` is not
                // rebound or written through between the guard and the use.
                narrowing::drop_unstable_members(&mut right_ctx, &[&*left, &*right]);
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
                    let peeled = narrowing::remove_nil(&lt);
                    // Not a union with the empty literal's open element: `list || []` answers list's type
                    let adopts = match (&*right.node, &peeled) {
                        (ExprNode::Array { elements, .. }, Ty::Array { .. }) => elements.is_empty() && !peeled.mentions_unknown(),
                        (ExprNode::Hash { entries, .. }, Ty::Hash { .. }) => entries.is_empty() && !peeled.mentions_unknown(),
                        _ => false,
                    };
                    if adopts {
                        right.ty = Some(peeled.clone());
                        return peeled;
                    }
                    peeled
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
                inner.class_objects.remove(name);
                inner.query_class_objects.remove(name);
                inner.live_class_locals.remove(name);
                if self.is_class_object(value, ctx) { inner.class_objects.insert(name.clone()); }
                if self.is_live_class(value, ctx) { inner.live_class_locals.insert(name.clone()); }
                self.analyze_expr(body, &inner)
            }

            ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. } => {
                let mut inner = ctx.clone();
                for name in params.iter().chain(rest_param.iter()).chain(block_param.iter()) {
                    inner.class_objects.remove(name);
                    inner.query_class_objects.remove(name);
                    inner.live_class_locals.remove(name);
                }
                if let Some(name) = self.block_class_param.borrow_mut().take().filter(|n| params.first() == Some(n)) {
                    inner.class_objects.insert(name);
                }
                for name in params.iter().chain(rest_param.iter()).chain(block_param.iter())
                    .chain(extra_params.iter().map(|param| &param.name))
                {
                    active_storage::forget_selector_proof(&mut inner, name);
                }
                if let Some((name, names)) = self.block_selector_param.borrow_mut().take().filter(|(n, _)| params.first() == Some(n)) {
                    active_storage::insert_selector_proof(&mut inner, &name, &names);
                }
                // Call-site evidence for a lambda stored in a local. Taken
                // here so a nested lambda does not consume it. Block
                // parameters still arrive through `ctx` from `block_ctx_for`.
                let seeds = self.lambda_seeds.borrow_mut().take();
                if let Some(seeds) = &seeds {
                    seeds.bind_positionals(&mut inner, params);
                }
                // Optional, keyword and keyword-rest parameters bind in the
                // body like any local: an optional as its default's type,
                // `**opts` as a Symbol-keyed Hash, a required keyword as
                // untyped until a `.call` site names it. A default is typed
                // in the lambda's own scope as filled so far, so it reads an
                // earlier extra parameter (`c = b`) rather than an outer local.
                for param in extra_params.iter_mut() {
                    let ty = match (&mut param.default, param.keyword && param.rest) {
                        (_, true) => Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Untyped) },
                        (Some(default), false) => self.analyze_expr(default, &inner),
                        (None, false) => Ty::Untyped,
                    };
                    if param.name.as_str().is_empty() {
                        continue;
                    }
                    inner.class_objects.remove(&param.name);
                    inner.query_class_objects.remove(&param.name);
                    inner.local_bindings.insert(param.name.clone(), ty);
                }
                if let Some(seeds) = &seeds {
                    seeds.bind_keywords(&mut inner, extra_params);
                }
                let body_ty = self.analyze_expr(body, &inner);
                // A block's parameter types were seeded into `ctx` and are
                // not repeated here. A stored lambda records the types its
                // `.call` sites supplied, so the value's type is
                // `^(Integer) -> Integer` rather than `^() -> untyped`.
                let fn_params = match &seeds {
                    Some(seeds) => seeds.fn_params(params),
                    None => Vec::new(),
                };
                Ty::Fn {
                    params: fn_params.into(),
                    block: None,
                    ret: std::sync::Arc::new(body_ty),
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
                let instance_receiver = recv.as_ref().map_or(ctx.instance_body, |r| self.is_instance(r, ctx))
                    && recv_ty.as_ref().is_some_and(instance_shaped);
                let dispatched = self.dispatch(recv_ty.as_ref(), name, None, &[]);
                if matches!(dispatched, Ty::Var { .. })
                    && let Some(Ty::Class { id, .. }) = recv_ty.as_ref()
                    && self.open_classes.is_some_and(|o| o.open(id, !instance_receiver).is_some())
                {
                    Ty::Untyped
                } else {
                    dispatched
                }
            }

            ExprNode::Apply { fun, args, block } => {
                self.analyze_expr(fun, ctx);
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                if let Some(b) = block { self.analyze_expr(b, ctx); }
                unknown()
            }

            ExprNode::Defined { .. } => union_of(Ty::Str, Ty::Nil),

            ExprNode::Send { recv, method, args, block, parenthesized } => {
                // Not an unknown Kernel call: `lambda { |x| … }` is the stabby `->(x) { … }`, whose value is the block (svg-graph's `DataPoint::DEFAULT_SHAPE`).
                // So is `proc { |x| … }`, whose value calls the same way.
                if recv.is_none()
                    && matches!(method.as_str(), "lambda" | "proc")
                    && args.is_empty()
                    && !self.owns_operator(ctx.self_ty.as_ref(), method, ctx.class_side)
                    && let Some(b) = block.as_mut().filter(|b| matches!(&*b.node, ExprNode::Lambda { .. }))
                {
                    return self.analyze_expr(b, ctx);
                }
                if let Some(ty) = self.class_factories.and_then(|factories| factories.get(&expr_span)) {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, None);
                    if let Some(recv) = recv { self.analyze_expr(recv, ctx); }
                    // A literal superclass is declaration metadata, just as
                    // in an ordinary class header. An external parent need
                    // not resolve as a supported value expression in main.
                    if let Ty::Class { id, .. } = ty
                        && let Some(parent) = self.classes.get(id).and_then(|info| info.parent.as_ref())
                    {
                        for arg in args.iter_mut() {
                            arg.ty = Some(Ty::Class { id: parent.clone(), args: vec![].into() });
                            arg.set_diagnostic(None);
                        }
                    }
                    return ty.clone();
                }
                if matches!(&crate::expr::ExprCold::diagnostic(&expr.cold), Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. })
                    if matches!(construct.as_str(), "Module protocol" | "ActiveModel::Errors" | "ActiveModel::Error" | "Rails" | "ActiveRecord::Base" | "String extension" | "Hash extension" | "not_nil!" | "Object extension" | "Module callback argument" | RUNTIME_MIXIN))
                { crate::expr::ExprCold::set_diagnostic(&mut expr.cold, None); }
                if self.inquirers.is_some()
                    && matches!(crate::expr::ExprCold::diagnostic(&expr.cold), Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. })
                        if construct.as_str() == "ActiveStorage constructor")
                {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, None);
                }
                expr.decisions &= !crate::expr::RESOLVED_DATA_FACTORY;
                // Not on a receiver's call: lowering's `Pathname.new(x)` keeps the mark analysis gave `Pathname(x)`.
                if recv.is_none() {
                    expr.decisions &= !crate::expr::KERNEL_CONVERSION;
                }
                if let Some(ty) = self.data_factories.and_then(|factories| factories.get(&expr_span)) {
                    // Only admitted declarations establish a Data class identity.
                    expr.decisions |= crate::expr::RESOLVED_DATA_FACTORY;
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, None);
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
                // `x.attr` under a guard that proved it non-nil (see
                // `narrowing::VarKey::Member`): the same read, typed from the guard.
                if args.is_empty() && block.is_none() {
                    if let Some(r) = recv.as_mut() {
                        if let ExprNode::Var { name, .. } = &*r.node {
                            if let Some(ty) = ctx.local_bindings.get(&narrowing::member_key(name, method)).cloned() {
                                self.analyze_expr(r, ctx);
                                return ty;
                            }
                        }
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
                    // Kernel#` - a backtick / `%x{}` shell-out (ingest
                    // spells the literal as this call): the command's
                    // standard output, a String, with `$?` set. An app class
                    // that defines its own ` is dispatched to it instead.
                    if method.as_str() == "`" && args.len() == 1 && !self.app_defines_on_side(ctx.self_ty.as_ref(), method, ctx.instance_body) {
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
                                // Not a class method from an instance body: `dom_id(self)` beside a `self.dom_id` is the instance's own.
                                (!ctx.instance_body && cls.class_methods.contains_key(method))
                                    || cls.instance_methods.contains_key(method))
                    );
                if resolves_through_self {
                    *recv = Some(crate::expr::Expr {
                        span: expr_span,
                        node: Box::new(ExprNode::SelfRef),
                        ty: ctx.self_ty.clone(),
                        effects: crate::effect::EffectSet::default(),
                        leading_blank_line: false,
                        cold: None,
                        decisions: 0,
                    });
                }

                let recv_ty = match recv.as_mut() {
                    Some(r) => Some(self.analyze_expr(r, ctx)),
                    None => ctx.self_ty.clone(),
                };
                // Not by name alone: an application `tap`/`each` may answer its block's value.
                if matches!(method.as_str(), "tap" | "each")
                    && let Some(b) = block.as_mut()
                    && let ExprNode::Lambda { body, .. } = &mut *b.node
                {
                    // Not left set once marked: an early round sees the receiver's class before its methods are known.
                    set_discarded(body, recv_ty.as_ref().is_some_and(|t| self.answers_receiver(t, method)));
                }
                if method.as_str() == "new"
                    && block.is_none()
                    && crate::analyze::indifferent::constructor_args(args)
                    && let Some(r) = recv.as_mut()
                    && matches!(&*r.node, ExprNode::Const { .. })
                    && matches!(&r.ty, Some(Ty::Class { id, args }) if args.is_empty() && id.0.as_str() == crate::analyze::indifferent::CLASS)
                {
                    r.set_diagnostic(None);
                }
                // Not a value: Rails' `call` answers the loaders, which `preload_associations` does not.
                if method.as_str() == "call" && args.is_empty() && block.is_none()
                    && expr.decisions & crate::expr::DISCARDED_VALUE != 0
                    && let Some(r) = recv.as_mut()
                    && crate::lower::preloader::preloader_new(r, |id| self.classes().get(id).is_some_and(|c| c.table.is_some())).is_some()
                {
                    crate::lower::preloader::clear_refusals(r);
                    expr.decisions |= crate::expr::ADMITTED_PRELOADER_CALL;
                    return Ty::Nil;
                }
                // Inside the class-side method, the instance a class-side
                // `new` built (`Ty::SelfInstance`, see below) answers the
                // methods of the class the `def` sits in: the receiving
                // subclass is only known at the call site, and what the
                // body can call on it is the base's surface. The value
                // itself stays `SelfInstance`, so the method still answers
                // the receiving class.
                let recv_ty = match (recv_ty, &ctx.self_ty) {
                    (Some(t), Some(self_ty @ Ty::Class { .. })) if ctx.class_side => {
                        Some(t.subst_self(self_ty))
                    }
                    (other, _) => other,
                };
                // `Parameters` is a Hash-shaped bag: what its own class does
                // not answer (`fetch`, `each`, `map`, `count`, ...) is the
                // Hash reading, over Symbol -> param value.
                let mut recv_ty = match recv_ty {
                    Some(Ty::Class { id, .. })
                        if id.0.as_str() == "ActionController::Parameters"
                            && method.as_str() != "new"
                            && !self.classes().get(&id).is_some_and(|c|
                                c.instance_methods.contains_key(method)
                                    || c.class_methods.contains_key(method)) =>
                    {
                        Some(send::params_as_hash())
                    }
                    other => other,
                };
                // Namespace owners exist so exception constants and class values work;
                // their empty runtime definitions do not implement Rails constructors.
                if self.inquirers.is_some()
                    && let Some(owner) = self.unsupported_active_storage_constructor(recv_ty.as_ref(), method, args, ctx)
                {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("ActiveStorage constructor"),
                        detail: format!("{owner}.new lacks a shared runtime constructor implementation"),
                    }));
                }
                // A self known only as some class or module (a `Class.new`
                // block, a singleton class): what its `new` builds has no name.
                if ctx.class_side
                    && matches!(method.as_str(), "new" | "allocate")
                    && recv.as_ref().map_or(true, |r| matches!(&*r.node, ExprNode::SelfRef))
                    && matches!(&recv_ty, Some(Ty::Class { id, args: targs }) if targs.is_empty() && matches!(id.0.as_str(), "Class" | "Module"))
                {
                    for a in args.iter_mut() {
                        self.analyze_expr(a, ctx);
                    }
                    return Ty::Untyped;
                }
                // `new` on the implicit or explicit `self` of a class-side
                // method builds an instance of the class that RECEIVED the
                // call: `CurrencyDb.load` runs `YamlDb.load` with `self ==
                // CurrencyDb`. Typing it as the class the `def` sits in made
                // every subclass factory answer the base class.
                if ctx.class_side
                    && method.as_str() == "new"
                    && recv.as_ref().map_or(true, |r| matches!(&*r.node, ExprNode::SelfRef))
                    && matches!(recv_ty, Some(Ty::Class { .. }))
                    && !self.has_declared_constructor(recv_ty.as_ref())
                {
                    for a in args.iter_mut() {
                        self.analyze_expr(a, ctx);
                    }
                    return Ty::SelfInstance;
                }
                for a in args.iter_mut() {
                    self.analyze_expr(a, ctx);
                }
                // The pagy gem's `pagy(scope)`: `[Pagy, <the scope's type>]`.
                if method.as_str() == "pagy"
                    && block.is_none()
                    && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                {
                    let mut refusal = None;
                    if let Some(ty) = self.pagy_call_ty(args, ctx, &mut refusal) {
                        if refusal.is_some() {
                            expr.set_diagnostic(refusal);
                        }
                        return ty;
                    }
                }
                if let Some(refusal) = self.pagy_object_refusal(recv_ty.as_ref(), method, args) {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(refusal));
                }
                if let Some(r) = recv.as_mut() {
                    if promotes_to_param_value(r, recv_ty.as_ref(), method, args, &ctx.local_bindings) {
                        r.ty = Some(send::param_value_ty());
                        recv_ty = r.ty.clone();
                    }
                }
                // Ruby's Module callback ABI supplies a class/module object. Direct
                // source calls must satisfy the same identity contract.
                if self.is_module_callback(recv_ty.as_ref(), method) {
                    if let Some(arg) = args.first() {
                        if !self.is_class_object(arg, ctx) {
                            crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                                target: None, construct: Symbol::from("Module callback argument"),
                                detail: format!("{} requires a proven class/module object argument", method),
                            }));
                        }
                    }
                }
                let class_object_receiver =
                    recv.as_ref().map_or(ctx.class_side, |r| self.is_class_object(r, ctx));
                let block_ret = if let Some(b) = block {
                    let mut block_ctx = self.block_ctx_for(
                        ctx,
                        recv_ty.as_ref(),
                        method,
                        args,
                        class_object_receiver,
                        b,
                    );
                    if matches!(method.as_str(), "instance_eval" | "instance_exec" | "class_eval" | "class_exec" | "module_eval" | "module_exec")
                        && !self.owns_operator(recv_ty.as_ref(), method, class_object_receiver)
                        && !(matches!(recv_ty.as_ref(), Some(Ty::Class { id, .. }) if matches!(id.0.as_str(), "Class" | "Module"))
                            && self.owns_operator(recv_ty.as_ref(), method, false))
                    {
                        if let Some(receiver) = recv.as_ref() {
                            block_ctx.self_ty = recv_ty.clone();
                            block_ctx.class_side = self.is_class_object(receiver, ctx);
                            block_ctx.instance_body = matches!(method.as_str(), "instance_eval" | "instance_exec")
                                && self.is_instance(receiver, ctx);
                        }
                    }
                    *self.block_selector_param.borrow_mut() = self.iterated_selectors(recv.as_ref(), method, b);
                    // Not any `new` with a block: only `Class.new`/`Module.new` makes the block's self a class it is building
                    if recv.as_ref().is_some_and(|r| self.class_factory_receiver(r, method, ctx))
                    {
                        expr.decisions |= crate::expr::RUNTIME_CLASS_OBJECT;
                        block_ctx.self_ty = recv_ty.clone();
                        block_ctx.class_side = true;
                        block_ctx.instance_body = false;
                        block_ctx.building_self = true;
                    }
                    // A method installed on that fresh class runs on its
                    // instances, not on the class that evaluated the factory.
                    if ctx.building_self && class_object_receiver && method.as_str() == "define_method"
                        && matches!(ctx.self_ty.as_ref(), Some(Ty::Class { id, .. }) if matches!(id.0.as_str(), "Class" | "Module"))
                        && !self.owns_operator(recv_ty.as_ref(), method, true)
                    {
                        block_ctx.self_ty = Some(Ty::Untyped);
                        block_ctx.class_side = false;
                        block_ctx.instance_body = true;
                        block_ctx.building_self = false;
                    }
                    let method_ref_ty = self.analyze_expr(b, &block_ctx);
                    self.block_selector_param.borrow_mut().take();
                    // The Lambda walker stores the analyzed body's type
                    // on the body expr itself. `map`/`collect`/similar
                    // use that to determine the output element type.
                    // `MethodRef` (`&method(:name)`) has no body to
                    // read from — its own computed type already IS
                    // the referenced method's return type.
                    match &*b.node {
                        ExprNode::Lambda { body, .. } => body.ty.clone(),
                        ExprNode::MethodRef { .. } => Some(method_ref_ty),
                        // Forwarded proc (`&callback`): Var in the block
                        // slot, no body to type. Presence must still reach
                        // dispatch — `PTY.spawn` with a block answers nil.
                        // A nil local (`callback = nil; …(&callback)`) is
                        // Ruby's no-block path, as is a literal `&nil`.
                        // A nilable local is refined for `PTY.spawn` after
                        // dispatch (Tuple | Nil); leave presence absent
                        // here so `String#bytes(&maybe)` stays the array.
                        ExprNode::Var { name, .. } => match ctx.local_bindings.get(name) {
                            Some(Ty::Nil) => None,
                            Some(ty) if forwarded_block_may_be_nil(ty) => None,
                            _ => Some(Ty::Untyped),
                        },
                        _ => None,
                    }
                } else {
                    None
                };
                if recv_ty.as_ref() == Some(&Ty::Str) && method.as_str() == "bytes"
                    && let Some(b) = block.as_ref()
                    && let ExprNode::Lambda { body, .. } = &*b.node
                    && bytes_block_has_escaping_break(body)
                {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("String#bytes block break"),
                        detail: "an escaping break can replace the receiver result; its return type is not modeled".into(),
                    }));
                }
                if method.as_str() == "new"
                    && matches!(&recv_ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "Data")
                    && self.const_resolver.as_ref().is_some_and(|resolver| !resolver.has_source_namespace("Data"))
                {
                    expr.set_diagnostic(Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("Data.new"),
                        detail: "Data is abstract; construct a class returned by Data.define".into(),
                    }));
                    return unknown();
                }
                let unsupported_deep_merge = match method.as_str() {
                    "deep_merge" if block.is_some() => Some((
                        "Hash#deep_merge conflict block",
                        "only the no-block Rails deep_merge form is supported",
                    )),
                    "deep_merge!" => Some((
                        "Hash#deep_merge!",
                        "the mutating deep_merge! form is unsupported",
                    )),
                    "deep_merge" if args.len() != 1 => Some((
                        "Hash#deep_merge arity",
                        "exactly one Hash argument is supported",
                    )),
                    "deep_merge"
                        if !matches!(args.first().and_then(|arg| arg.ty.as_ref()), Some(Ty::Hash { .. })) =>
                    {
                        Some((
                            "Hash#deep_merge argument",
                            "the supported no-block form requires a statically typed Hash argument",
                        ))
                    }
                    _ => None,
                };
                if let Some((construct, detail)) = unsupported_deep_merge
                    && matches!(recv_ty, Some(Ty::Hash { .. }))
                {
                    expr.set_diagnostic(Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from(construct),
                        detail: detail.into(),
                    }));
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
                if let Some(t) = self.assoc_loaded_ty(recv.as_ref(), method) {
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
                if let Some(t) = recv.as_ref().and_then(|recv| send::literal_array_extreme_ty(recv, method, args, block.is_some())) {
                    return t;
                }
                if let Some(t) = self.relation_extreme_ty(
                    recv.as_ref(),
                    recv_ty.as_ref(),
                    method,
                    args,
                    ctx.instance_body.then_some(ctx.self_ty.as_ref()).flatten(),
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
                if let Some(t) = expect_hash_arg_ty(recv.as_ref(), recv_ty.as_ref(), method.as_str(), args) {
                    return t;
                }
                // Not `Integer?`: `nil.to_fs` raises in Rails, so only a number that cannot be nil types.
                if matches!(recv_ty, Some(Ty::Int) | Some(Ty::Float))
                    && crate::lower::number_to_fs::is_delimited_to_fs(method, args)
                {
                    return Ty::Str;
                }
                if let Some(t) = recv_ty
                    .as_ref()
                    .and_then(|t| literal_extremum_ty(recv.as_ref(), t, method, args))
                {
                    return t;
                }
                if expr.decisions & crate::expr::REVERSE_MERGE != 0
                    && let (Some(defaults), [hash]) = (recv_ty.as_ref(), args.as_slice())
                    && let Some(ty) = hash.ty.as_ref().and_then(|h| crate::analyze::indifferent::reverse_merged(defaults, h))
                {
                    return ty;
                }
                if let (Some(receiver), Some(materialized)) = (recv.as_ref(), self.materialized_associations)
                    && let Some(columns) = super::assoc_pluck::reader_columns(receiver, method, args, block.as_ref(), materialized)
                    && let Some(projected) = receiver.ty.as_ref().and_then(|ty| self.reader_projection(ty, &columns))
                {
                    return projected;
                }
                // Only the constant `Class` builds a class with `new`. A class object
                // of unknown identity (`x.class`, campfire's `sanitizer_class`) builds
                // an instance of whichever class it is; answering `Class` handed
                // spinel a Class receiver for the instance's own calls.
                if method.as_str() == "new"
                    && matches!(recv_ty.as_ref(), Some(Ty::Class { id, args: targs }) if id.0.as_str() == "Class" && targs.is_empty())
                    && recv.as_ref().is_some_and(|r| !matches!(&*r.node, ExprNode::Const { .. }))
                {
                    return Ty::Untyped;
                }
                let instance_receiver = recv.as_ref().map_or(ctx.instance_body, |r| self.is_instance(r, ctx))
                    && recv_ty.as_ref().is_some_and(instance_shaped);
                let mut dispatched =
                    self.dispatch_on(recv_ty.as_ref(), method, block_ret.as_ref(), args, instance_receiver);
                // An `app/helpers` method runs in the view, so a bare call that finds nothing on the module is the
                // view's: the helper module that answers the name (`helpers :all`; the module itself while its own
                // definition is untyped), else ActionView's `link_to`/`tag`/`image_tag`.
                if recv.is_none()
                    && !ctx.class_side
                    && matches!(dispatched, Ty::Var { .. })
                    && let Some(Ty::Class { id: helper, .. }) = ctx.self_ty.as_ref()
                    && self.helper_modules.is_some_and(|h| h.contains(helper))
                {
                    let owner = self.helper_index.and_then(|index| index.get(method)).cloned();
                    let view = owner.unwrap_or_else(|| ClassId(Symbol::from("ActionView::Base")));
                    let alt = self.dispatch_on(Some(&Ty::Class { id: view, args: vec![].into() }), method, block_ret.as_ref(), args, true);
                    if !matches!(alt, Ty::Var { .. }) {
                        dispatched = alt;
                    }
                }
                // Not a dispatch miss: a class that defines methods under computed names answers what its own code defines when it runs.
                if matches!(dispatched, Ty::Var { .. })
                    && let Some(Ty::Class { id, .. }) = recv_ty.as_ref()
                    && self.open_classes.is_some_and(|o| o.open(id, !instance_receiver).is_some())
                {
                    dispatched = Ty::Untyped;
                }
                // Not left to the positional constructor: since Ruby 3.2 a Struct takes keywords too (bound by member name), which the emitted positional constructor would bind to its first member.
                if method.as_str() == "new"
                    && crate::expr::ExprCold::diagnostic(&expr.cold).is_none()
                    && matches!(args.last().map(|a| &*a.node), Some(ExprNode::Hash { kwargs: true, .. }))
                    && matches!(recv_ty.as_ref(), Some(Ty::Class { id, .. }) if self.classes.get(id).is_some_and(|c| c.positional_struct))
                {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("Struct keyword construction"),
                        detail: "keyword construction of a `Struct.new(...)` without `keyword_init:` is not modeled: the emitted constructor is positional and would bind the keywords to its first member".to_string(),
                    }));
                }
                // `PTY.spawn(..., &maybe)` when `maybe` is nilable: Ruby
                // may take the block (nil) or not (tuple). Presence was
                // left absent above so other methods keep their no-block
                // answer; widen the spawn result here.
                if method.as_str() == "spawn"
                    && matches!(&recv_ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "PTY")
                    && let Some(b) = block.as_ref()
                    && let ExprNode::Var { name, .. } = &*b.node
                    && ctx
                        .local_bindings
                        .get(name)
                        .is_some_and(forwarded_block_may_be_nil)
                {
                    let file = Ty::Class { id: ClassId(Symbol::from("File")), args: vec![].into() };
                    return union_of(
                        Ty::Tuple { elems: vec![file.clone(), file, Ty::Int].into() },
                        Ty::Nil,
                    );
                }
                if let Some(receiver) = recv.as_mut() {
                    receiver.decisions &= !crate::expr::RESOLVED_OPERATOR_RECEIVER;
                    if matches!(method.as_str(), "+" | "-" | "*" | "/" | "**" | "%" | "<" | "<=" | ">" | ">=")
                        && self.owns_operator(recv_ty.as_ref(), method, self.is_class_object(receiver, ctx))
                    {
                        receiver.decisions |= crate::expr::RESOLVED_OPERATOR_RECEIVER;
                    }
                }
                // `include ActionView::RecordIdentifier` is a marker include
                // (dropped at ingest); emit qualifies only the BARE
                // `dom_id` / `dom_class` to `ViewHelpers`, so only the bare
                // call types String. An explicit receiver (`x.dom_id(a)`)
                // has nothing to run and stays unresolved.
                if recv.is_none()
                    && matches!(method.as_str(), "dom_id" | "dom_class")
                    && (1..=2).contains(&args.len())
                    && block.is_none()
                    && matches!(dispatched, Ty::Var { .. })
                    && !self.app_defines_on_side(ctx.self_ty.as_ref(), method, ctx.instance_body)
                {
                    return Ty::Str;
                }
                if recv.is_none()
                    && matches!(method.as_str(), "render" | "render_to_string")
                    && ctx.helper_body
                    && !ctx.class_side
                    && !self.app_defines_on_side(ctx.self_ty.as_ref(), method, ctx.instance_body)
                {
                    return Ty::Str;
                }
                // Kernel.Array is a container even for scalar params.
                // App methods (including inherited/included overrides)
                // have already dispatched above and must win.
                // RBS declares it `(untyped) -> Array[untyped]`; the
                // argument says more.
                // Not a Float argument: CRuby's `BigDecimal(Float)` needs a precision and spinel's package has none.
                if recv.is_none() && method.as_str() == "BigDecimal" && args.len() == 1 && block.is_none()
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && matches!(args[0].ty.as_ref(), Some(Ty::Str | Ty::Int))
                    && !self.app_defines(ctx.self_ty.as_ref(), method)
                {
                    return send::bigdecimal();
                }
                // Not with a Hash argument: `exception: false` answers nil where the conversion fails.
                if recv.is_none() && block.is_none()
                    && ((method.as_str() == "Integer" && matches!(args.len(), 1 | 2)) || (method.as_str() == "Float" && args.len() == 1))
                    && !args.iter().any(|a| matches!(&*a.node, ExprNode::Hash { .. }))
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && !self.app_defines(ctx.self_ty.as_ref(), method)
                {
                    return if method.as_str() == "Integer" { Ty::Int } else { Ty::Float };
                }
                // Kernel's other conversions, never nil under the same rule. `Rational(...)` and
                // `URI(str)` stay gradual, like `to_r` and `URI.parse`: the registry models neither
                // result. Only a String makes `Pathname(path)` the `Pathname.new(path)` lowering
                // writes: given a Pathname, Kernel's answers that same object, frozen or not.
                // Only a method on the side the call dispatches to shadows Kernel's: a class
                // body's call does not reach an included module's instance method.
                if recv.is_none() && block.is_none()
                    && matches!((method.as_str(), args.len()), ("Pathname" | "URI", 1) | ("Rational", 1 | 2))
                    && !args.iter().any(|a| matches!(&*a.node, ExprNode::Hash { .. }))
                    && (method.as_str() != "Pathname" || matches!(args[0].ty, Some(Ty::Str)))
                    && !self.app_defines_on_side(ctx.self_ty.as_ref(), method, ctx.instance_body)
                    && (matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                        || self.app_defines_on_side(ctx.self_ty.as_ref(), method, !ctx.instance_body))
                {
                    expr.decisions |= crate::expr::KERNEL_CONVERSION;
                    return match method.as_str() {
                        "Pathname" => Ty::Class { id: ClassId(Symbol::from("Pathname")), args: vec![].into() },
                        _ => Ty::Untyped,
                    };
                }
                if recv.is_none() && method.as_str() == "Array" && args.len() == 1
                    && block.is_none()
                    && (matches!(dispatched, Ty::Var { .. })
                        || (matches!(dispatched, Ty::Untyped)
                            && !self.app_defines(ctx.self_ty.as_ref(), method)))
                {
                    let elem = args[0].ty.as_ref().and_then(kernel_array_elem);
                    return Ty::Array { elem: std::sync::Arc::new(elem.unwrap_or_else(unknown)) };
                }
                let translate = matches!(method.as_str(), "t" | "translate");
                let catalog = self.i18n.map_or(crate::i18n::Catalog::rails_default(), |c| c.or_rails_default());
                let scope = match (!translate || ctx.in_view, recv.is_none() && !ctx.class_side) {
                    (false, true) => self.translating_controller(ctx.self_ty.as_ref()),
                    _ => None,
                };
                // An `app/helpers` module runs in the view, so its `t` is ActionView's (with no template for a lazy key).
                let in_helper = !ctx.class_side
                    && matches!(ctx.self_ty.as_ref(), Some(Ty::Class { id, .. }) if self.helper_modules.is_some_and(|h| h.contains(id)));
                let helper_copy = expr.decisions & (crate::expr::HELPER_COPY_TRANSLATE | crate::expr::HELPER_BODY_TRANSLATE) != 0;
                let site = match (&scope, ctx.in_view) {
                    (_, true) => crate::lower::i18n_translate::Site::View(ctx.view_name.as_ref().map(|v| v.as_str())),
                    _ if helper_copy => crate::lower::i18n_translate::Site::View(None),
                    (None, false) if in_helper => crate::lower::i18n_translate::Site::View(None),
                    (Some(scope), false) => crate::lower::i18n_translate::Site::Controller(scope),
                    (None, false) => crate::lower::i18n_translate::Site::Other,
                };
                if translate
                    && crate::lower::i18n_translate::tree_answer(catalog, recv.as_ref(), method.as_str(), args, block.as_ref()).is_some()
                {
                    return Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Untyped) };
                }
                if translate
                    && let Some((_, _, ty)) = crate::lower::i18n_translate::value_answer(catalog, recv.as_ref(), method.as_str(), args, block.as_ref(), site)
                {
                    return ty;
                }
                if translate && let Some(resolved) = crate::lower::i18n_translate::resolves(
                    catalog, recv.as_ref(), method.as_str(), args, block.as_ref(), site,
                ) {
                    if let Err(reason) = resolved {
                        if crate::expr::ExprCold::diagnostic(&expr.cold).is_none() {
                            crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                                target: None,
                                construct: Symbol::from("I18n"),
                                detail: format!("`{method}` cannot be resolved from the app's locale: {reason}"),
                            }));
                        }
                    }
                    return Ty::Str;
                }
                if matches!(recv_ty.as_ref(), Some(Ty::Class { args: model, .. }) if !model.is_empty())
                    && crate::lower::view::wraps_field_errors(method.as_str())
                    && crate::expr::ExprCold::diagnostic(&expr.cold).is_none()
                {
                    let catalog = self.i18n.map_or(crate::i18n::Catalog::rails_default(), |c| c);
                    if let Some(file) = &catalog.field_error_proc {
                        crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                            target: None,
                            construct: Symbol::from("field_with_errors"),
                            detail: format!("`{method}` renders through the `field_error_proc` {file} sets, which is not modeled"),
                        }));
                    }
                }
                if method.as_str() == "human_attribute_name"
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && crate::lower::human_attribute_name::literal_attribute(args).is_some()
                    && match recv.as_ref() {
                        Some(r) => crate::lower::human_attribute_name::receiver_model(r),
                        None if ctx.class_side => match ctx.self_ty.as_ref() {
                            Some(Ty::Class { id, .. }) => Some(id.clone()),
                            _ => None,
                        },
                        None => None,
                    }
                    .is_some_and(|id| self.classes.get(&id).is_some_and(|c| c.table.is_some()))
                {
                    return Ty::Str;
                }
                if method.as_str() == "where"
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && matches!(recv_ty.as_ref(), Some(Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Errors")
                    && block.is_none()
                    && crate::lower::errors_index::where_args(args).is_some()
                {
                    return crate::lower::errors_index::error_array_ty();
                }
                if method.as_str() == "delete"
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && matches!(recv_ty.as_ref(), Some(Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Errors")
                    && block.is_none()
                    && crate::lower::errors_index::delete_field(args).is_some()
                {
                    return Ty::Union { variants: vec![Ty::Array { elem: std::sync::Arc::new(Ty::Str) }, Ty::Nil].into() };
                }
                if matches!(method.as_str(), "import" | "merge!")
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && matches!(recv_ty.as_ref(), Some(Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Errors")
                    && block.is_none()
                    && crate::lower::errors_add::import_grounds(method.as_str(), args, &|id| self.classes.get(id).is_some_and(|c| c.table.is_some()))
                {
                    return crate::lower::errors_index::error_array_ty();
                }
                if method.as_str() == "[]"
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && matches!(recv_ty.as_ref().map(Ty::peel_nilable), Some(Ty::Class { id, .. }) if id.0.as_str() == crate::lower::errors_index::DETAILS_ENTRY)
                    && crate::lower::errors_index::reads_detail_type(args)
                {
                    return Ty::Union { variants: vec![Ty::Sym, Ty::Str].into() };
                }
                if matches!(method.as_str(), "of_kind?" | "added?")
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && matches!(recv_ty.as_ref(), Some(Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Errors")
                    && block.is_none()
                    && crate::lower::errors_index::of_kind_grounds(args)
                {
                    return Ty::Bool;
                }
                if method.as_str() == "full_message"
                    && matches!(dispatched, Ty::Var { .. } | Ty::Untyped)
                    && matches!(recv_ty.as_ref(), Some(Ty::Class { id, .. }) if id.0.as_str() == "ActiveModel::Errors")
                    && crate::lower::errors_add::full_message_bakes(recv.as_ref(), args, block.as_ref())
                {
                    return Ty::Str;
                }
                // What every object and every module answers, when the
                // receiver's own table did not. App analyzer only.
                // `class_object_receiver` was resolved above for block binding
                // so it matches the same class/instance table preference. A
                // `Module` subclass's own implicit-self Module protocol is a
                // class-object receiver too.
                let class_object_receiver = class_object_receiver
                    || (recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                        && send::is_module_protocol(method) && self.self_is_module_instance(ctx));
                let live_class = recv.as_ref().map_or(ctx.class_side && !ctx.building_self, |r| self.is_live_class(r, ctx));
                if ctx.method_body && live_class && self.inquirers.is_some() && !ctx.claimed_macro_template
                    && matches!(method.as_str(), "include" | "prepend" | "extend")
                    && !self.owns_operator(recv_ty.as_ref(), method, true)
                {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from(RUNTIME_MIXIN),
                        detail: format!("`{}` in a method mixes a module into a live class, which needs a method table \
                                         that changes while the program runs; no compiled target has one", method.as_str()),
                    }));
                    return dispatched;
                }
                if matches!(dispatched, Ty::Var { .. } | Ty::Untyped) && self.inquirers.is_some()
                    && (recv.is_some() || (ctx.self_ty.is_some() && (send::is_module_protocol(method)
                        || (method.as_str() == "singleton_class"
                            && self.has_singleton_class(ctx.self_ty.as_ref(), ctx.class_side, method)))))
                    && !self.owns_operator(recv_ty.as_ref(), method, class_object_receiver) {
                    let class_object = class_object_receiver
                        || (send::is_module_query(method) && recv.as_ref().is_some_and(|r| self.answers_module_queries(r, recv_ty.as_ref(), ctx)));
                    if class_object && send::is_module_query(method) {
                        expr.decisions |= crate::expr::MODULE_QUERY;
                    }
                    // `include?` is Module's ancestor check AND String/Enumerable
                    // membership. Refuse Module protocol only for true Module-only
                    // names, or for `include?` on a nominal class *instance* (where
                    // Module#include? would be the wrong answer). Untyped /
                    // String / Array receivers fall through to membership → Bool, and
                    // so does a value typed `Class` or `Module`, which is a module
                    // object and answers Module#include? itself.
                    let module_only = send::is_module_protocol(method)
                        && (method.as_str() != "include?"
                            || matches!(&recv_ty, Some(Ty::Class { id, .. })
                                if !matches!(id.0.as_str(), "Class" | "Module") && !self.descends_from_collection(id)));
                    if module_only && !class_object
                        && !self.owns_operator(recv_ty.as_ref(), method, false) {
                        crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                            target: None,
                            construct: Symbol::from("Module protocol"),
                            detail: format!("{} requires a proven class/module object receiver", method),
                        }));
                        return dispatched;
                    }
                    if let Some(t) = send::object_protocol_method(recv_ty.as_ref(), method, block_ret.as_ref(), class_object) {
                        return t;
                    }
                }
                if matches!(dispatched, Ty::Var { .. } | Ty::Untyped) && self.inquirers.is_some()
                    && !self.owns_operator(recv_ty.as_ref(), method, class_object_receiver) {
                    let gap = match super::singleton_of(recv_ty.as_ref()).or(recv_ty.as_ref()) {
                        Some(Ty::Class { id, .. }) if matches!(id.0.as_str(), "ActiveModel::Errors" | "ActiveModel::Error") => Some(id.0.as_str()),
                        Some(Ty::Class { id, .. })
                            if matches!(id.0.as_str(), "Rails" | "ActiveRecord::Base")
                                && !ctx.claimed_macro_template =>
                        {
                            Some(id.0.as_str())
                        }
                        Some(Ty::Str) if matches!(method.as_str(), "to_date" | "to_time" | "to_datetime" | "in_time_zone" | "to_d" | "as_json" | "tableize" | "strip_heredoc") => Some("String extension"),
                        Some(Ty::Hash { .. }) if method.as_str() == "to_sentence" => Some("Hash extension"),
                        _ if method.as_str() == "not_nil!" => Some("not_nil!"),
                        // `to_param` on Untyped goes through ActiveSupport.to_param
                        // (residue lowering + runtime). Refuse only on a nominal
                        // class with no modeled/synthesized reader (e.g. a plain
                        // PORO). Untyped helper params (campfire AvatarsHelper)
                        // must not hard-fail.
                        _ if method.as_str() == "to_param"
                            && matches!(recv_ty, Some(Ty::Class { .. })) =>
                        {
                            Some("Object extension")
                        }
                        // Not refused again on a receiver the constant refusal already accounts for.
                        _ if matches!(recv_ty, None | Some(Ty::Var { .. } | Ty::Untyped))
                            && recv.as_ref().is_some_and(|r| rooted_in_refused_constant(r)) => None,
                        // Grounded, so not a gap: `presence_in(<Array[String]>)` is
                        // `ActiveSupport.presence_in` (lower::presence_in), and a
                        // zero-arg `to_query` on a Hash, or on a receiver inference
                        // could not type, is `ActiveSupport.to_query` (lower::symbolize_keys).
                        // A list inference cannot prove is an Array[String], or a
                        // `to_query(key)` (the Object/Array/nil form), still refuse.
                        _ if method.as_str() == "presence_in" && args.len() == 1 && block.is_none() && is_string_array(args[0].ty.as_ref()) => None,
                        _ if method.as_str() == "to_query" && args.is_empty() && block.is_none()
                            && matches!(recv_ty, None | Some(Ty::Var { .. } | Ty::Untyped | Ty::Hash { .. })) => None,
                        _ if matches!(method.as_str(), "to_query" | "instance_values" | "acts_like?" | "presence_in" | "as_json" | "with_options" | "pretty_inspect") => Some("Object extension"),
                        _ => None,
                    };
                    // Not an Object extension under an unseen ancestor: a gem likely defines it, and only a dispatch miss keeps the receiver attribution needs.
                    let gem_ancestry = recv_ty
                        .clone()
                        .filter(|ty| gap == Some("Object extension") && matches!(ty, Ty::Class { id, .. } if self.has_unknown_ancestor(id)));
                    if let Some(recv_ty) = gem_ancestry.filter(|_| crate::expr::ExprCold::diagnostic(&expr.cold).is_none()) {
                        crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::SendDispatchFailed {
                            method: method.clone(),
                            recv_ty,
                        }));
                    } else if let Some(owner) = gap.filter(|_| crate::expr::ExprCold::diagnostic(&expr.cold).is_none()) {
                        crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                            target: None,
                            construct: Symbol::from(owner),
                            detail: format!("{} lacks a shared runtime/lowering implementation", method),
                        }));
                    }
                }
                // `sanitize_sql_for_order` is modeled on a String only: the
                // fragment unchanged. Any other argument may be Rails' Array
                // form, which raises for a `?` fragment there; refuse it, on
                // Base and on every model, implicit self included.
                if method.as_str() == "sanitize_sql_for_order"
                    && self.is_model_class(recv_ty.as_ref())
                    && !(args.len() == 1 && block.is_none() && matches!(args[0].ty, Some(Ty::Str)))
                    && crate::expr::ExprCold::diagnostic(&expr.cold).is_none()
                {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("ActiveRecord::Base"),
                        detail: "sanitize_sql_for_order is modeled on a String only; the Array form is not".to_string(),
                    }));
                }
                // The runtime jar's `each` yields unconditionally; Rails'
                // blockless form answers an Enumerator, which it does not.
                if method.as_str() == "each"
                    && block.is_none()
                    && matches!(recv_ty.as_ref(), Some(Ty::Class { id, .. }) if id.0.as_str() == "ActionDispatch::Cookies::CookieJar")
                    && crate::expr::ExprCold::diagnostic(&expr.cold).is_none()
                {
                    crate::expr::ExprCold::set_diagnostic(&mut expr.cold, Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("ActionDispatch::Cookies::CookieJar"),
                        detail: "cookies.each is modeled with a block only; the blockless Enumerator is not".to_string(),
                    }));
                }
                dispatched
            }

            ExprNode::If { cond, then_branch, else_branch } => {
                self.analyze_expr(cond, ctx);
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
                let mut cond_assigns: HashMap<Symbol, Ty> = HashMap::default();
                collect_var_assignments_into(cond, &mut cond_assigns);
                let mut base = ctx.clone();
                for (k, v) in &cond_assigns {
                    base.local_bindings.insert(k.clone(), v.clone());
                }
                self.propagate_match_bindings(cond, &mut base, true);
                let reader = |n: &Symbol| self.self_attribute_ty(n, ctx);
                let member = |l: &Symbol, a: &Symbol| self.member_attribute_ty(l, a, &base);
                let pred = narrowing::extract_narrowing_with(cond, &narrowing::Probe { reader: &reader, member: &member });
                let mut then_ctx = match &pred {
                    Some(p) => narrowing::apply_narrowing(&base, p, true),
                    None => base.clone(),
                };
                narrowing::drop_unstable_members(&mut then_ctx, &[&*cond, &*then_branch]);
                let t = self.analyze_expr(then_branch, &then_ctx);
                let mut else_ctx = match &pred {
                    Some(p) => narrowing::apply_narrowing(&base, p, false),
                    None => base.clone(),
                };
                narrowing::drop_unstable_members(&mut else_ctx, &[&*cond, &*else_branch]);
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
                let mut array_seed_idx: HashMap<(bool, Symbol), usize> = HashMap::default();
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
                let mut nil_seed: HashMap<Symbol, (usize, Ty)> = HashMap::default();
                // Statement index of each live empty-hash seed (`attrs =
                // {}`) — the Hash counterpart of `array_seed_idx`.
                let mut hash_seed_idx: HashMap<(bool, Symbol), usize> = HashMap::default();
                // Lambdas assigned to locals in this sequence. Later
                // `.call` sites refine their parameter types and the
                // assignment is retyped. A reassignment drops the entry.
                let mut lambda_locals: HashMap<Symbol, lambda_local::Stored> = HashMap::default();
                for i in 0..exprs.len() {
                    let read_structurally_later = match &*exprs[i].node {
                        ExprNode::Assign { target: LValue::Var { name, .. }, .. } => {
                            exprs[i + 1..].iter().any(|later| reads_structurally(later, name))
                        }
                        _ => false,
                    };
                    let tail = i + 1 == exprs.len();
                    // What the statement was typed against, for the retype below.
                    let entry_ctx = (!nil_seed.is_empty()).then(|| local_ctx.clone());
                    {
                        let e = &mut exprs[i];
                        if !tail {
                            mark_discarded(e);
                        }
                        if !matches!(&*e.node, ExprNode::Assign { target: LValue::Var { .. }, .. }) {
                            forget_class_object_writes(e, &mut local_ctx);
                        }
                        last = self.analyze_expr(e, &local_ctx);
                        // A self binding gives `self` its declared type from
                        // the next statement to the end of this body, blocks
                        // inside included. It changes only how `self` types:
                        // which class a method defined below belongs to is
                        // settled at ingest, lexically. A class-object binding
                        // puts `self` on the class side, as in a `def self.x`.
                        if crate::expr::is_self_binding(e) {
                            if local_ctx.lexical_self.is_none() {
                                local_ctx.lexical_self = local_ctx.self_ty.clone();
                            }
                            local_ctx.self_ty = e.ty.clone();
                            local_ctx.class_side = e.decisions & crate::expr::SELF_BINDING_CLASS_OBJECT != 0;
                        }
                        if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*e.node {
                            let proven = self.is_class_object(value, &local_ctx);
                            let query = !proven && self.each_branch_is_class_object(value, &local_ctx);
                            let live = self.is_live_class(value, &local_ctx);
                            forget_class_object_writes(value, &mut local_ctx);
                            local_ctx.class_objects.remove(name);
                            local_ctx.query_class_objects.remove(name);
                            local_ctx.live_class_locals.remove(name);
                            if proven { local_ctx.class_objects.insert(name.clone()); }
                            // `flag ? A : B` has no single identity to write
                            // through, but either class answers a Module query.
                            else if query {
                                local_ctx.query_class_objects.insert(name.clone());
                            }
                            self.prove_selector(name, value, &mut local_ctx);
                            if live { local_ctx.live_class_locals.insert(name.clone()); }
                            // Before the binding insert below: the lambda body
                            // was typed in this `local_ctx`, without its own name.
                            lambda_local::remember(&mut lambda_locals, i, e, &local_ctx);
                        }
                    }
                    // Before the binding insert and the seed/narrowing
                    // epilogue. Those read this statement's types, and a
                    // `.call` in it may have just given the lambda its
                    // parameter types.
                    let retired = lambda_local::forget_nested_writes(&mut lambda_locals, exprs, i);
                    // A join reads the binding it was given for the side
                    // that does not write the name. Dropping a write
                    // `flow_locals` can see makes that side nil. A write it
                    // cannot see (a block) leaves the old proc in place, and
                    // a later `.call` returns the proc's result.
                    let joins = matches!(
                        &*exprs[i].node,
                        ExprNode::If { .. }
                            | ExprNode::Case { .. }
                            | ExprNode::While { .. }
                            | ExprNode::BeginRescue { .. }
                            | ExprNode::BoolOp { .. }
                    );
                    let mut unknown_join = crate::det::HashSet::default();
                    if joins {
                        let mut flowed = HashMap::default();
                        let _ = flow_locals(&exprs[i], &local_ctx.local_bindings, &mut flowed, &mut unknown_join);
                    }
                    for name in retired {
                        if !joins || unknown_join.contains(&name) {
                            local_ctx.local_bindings.remove(&name);
                        }
                    }
                    self.refine_lambda_locals(exprs, i, &mut local_ctx, &mut lambda_locals, &mut last);
                    let e = &mut exprs[i];
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
                    // A branching STATEMENT (`if`/`unless`/ternary,
                    // `case`, `while`, `begin … rescue`, `a && (x = …)`)
                    // exports the locals it assigns: Ruby's control flow
                    // opens no scope, so a local assigned on one path is
                    // bound after the join on every path, holding nil
                    // where it was never assigned. [`flow_locals`] walks
                    // the paths and joins them. Without this, `if c; x =
                    // 1; end; x` read `x` as unresolved, and `w = 0; if
                    // c; w = "s"; end; w` read `w` as Int, which is wrong.
                    // The `begin` case was found by the model scenario
                    // scripts/campfire-db-differential runs statement by
                    // statement (`user = User.find(id)` in a begin,
                    // `user.messages.create!` below).
                    let mut flow_exported = crate::det::HashSet::default();
                    if matches!(&*e.node,
                        ExprNode::If { .. } | ExprNode::Case { .. } | ExprNode::While { .. }
                            | ExprNode::BeginRescue { .. } | ExprNode::BoolOp { .. })
                    {
                        let mut flowed = HashMap::default();
                        let mut unknown = crate::det::HashSet::default();
                        if flow_locals(e, &local_ctx.local_bindings, &mut flowed, &mut unknown) {
                            for (name, ty) in flowed {
                                if !unknown.contains(&name) {
                                    flow_exported.insert(name.clone());
                                    local_ctx.local_bindings.insert(name, ty);
                                }
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
                    // `instance_variable_set(:@article, record)` is Kernel,
                    // not `Assign`, so the arm above misses it. Fold a
                    // statically resolvable name so a later statement in
                    // the same method can read the ivar.
                    if let Some((name, ty)) =
                        super::ivar_set::binding_from_send(e, local_ctx.self_ty.as_ref())
                    {
                        local_ctx.ivar_bindings.insert(name, ty);
                    }
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
                                value: std::sync::Arc::new(new_value),
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
                            let refined = Ty::Array { elem: std::sync::Arc::new(new_elem) };
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
                        let mut widened: Vec<(Symbol, Ty)> = Vec::new();
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
                            seed.ty = Some(merged.clone());
                            widened.push((name.clone(), merged));
                        }
                        // Not left as first typed: a read earlier in the same loop body (`if prev … prev = x`) saw the bare nil seed, and a body typed only once (a view) keeps that.
                        // Not against the context after the statement either: its other writes would reach reads that precede them.
                        if !widened.is_empty() && si_is_not(i, &nil_seed) {
                            if let Some(mut retype) = entry_ctx {
                                for (name, ty) in widened {
                                    retype.local_bindings.insert(name, ty);
                                }
                                let first = match &*exprs[i].node {
                                    ExprNode::Assign { target: LValue::Var { .. }, value } => value.ty.clone(),
                                    _ => None,
                                };
                                last = self.analyze_expr(&mut exprs[i], &retype);
                                // Not the binding the first typing exported: `values = xs.map { … prev … }` now answers the widened element.
                                // Not over a refinement either: an accumulator scan may already have narrowed the binding past the first typing.
                                if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*exprs[i].node {
                                    if let Some(ty) = value.ty.clone().filter(|t| !t.is_open()) {
                                        if !nil_seed.contains_key(name) && local_ctx.local_bindings.get(name) == first.as_ref() {
                                            local_ctx.local_bindings.insert(name.clone(), ty);
                                        }
                                    }
                                }
                            }
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
                    // a good prior binding isn't clobbered, and so is a name
                    // [`flow_locals`] already joined: its answer includes the
                    // branches' own writes and what the condition proved.
                    if let ExprNode::If { cond, .. } | ExprNode::While { cond, .. } =
                        &*e.node
                    {
                        let mut cond_assigns: HashMap<Symbol, Ty> = HashMap::default();
                        collect_var_assignments_into(cond, &mut cond_assigns);
                        for (name, ty) in cond_assigns {
                            if ty.is_open() || flow_exported.contains(&name) {
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
                            if let Some(pred) = narrowing::extract_narrowing_with(cond, &narrowing::Probe::without_members(&|n| self.self_attribute_ty(n, ctx))) {
                                local_ctx = narrowing::apply_narrowing(&local_ctx, &pred, false);
                            }
                        }
                        // The mirror image is `unless`, which the ingest
                        // spells `if cond; <nothing>; else <diverge>; end`:
                        // `return false unless current` continues only when
                        // `current` was truthy.
                        let then_empty = match &*then_branch.node {
                            ExprNode::Lit { value: crate::expr::Literal::Nil } => true,
                            ExprNode::Seq { exprs } => exprs.is_empty(),
                            _ => false,
                        };
                        let else_diverges = matches!(else_branch.ty.as_ref(), Some(Ty::Bottom));
                        if else_diverges && then_empty {
                            if let Some(pred) = narrowing::extract_narrowing_with(cond, &narrowing::Probe::without_members(&|n| self.self_attribute_ty(n, ctx))) {
                                local_ctx = narrowing::apply_narrowing(&local_ctx, &pred, true);
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

            ExprNode::ForwardArgs | ExprNode::ForwardKeywords => Ty::Untyped,
            ExprNode::ForwardKeywordsWithPairs { entries } => {
                for (key, value) in entries.iter_mut() {
                    self.analyze_expr(key, ctx);
                    self.analyze_expr(value, ctx);
                }
                Ty::Untyped
            }

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
                    args: vec![elem].into(),
                }
            }
            ExprNode::Cast { value, target_ty } => {
                // Visit the value so its inner sub-exprs get typed,
                // then return the explicit cast target — that's the
                // whole point of Cast: assert this expression has
                // `target_ty` regardless of what the value's flow
                // type computed to.
                let _ = self.analyze_expr(value, ctx);
                // A class named inside a namespace means the lexically
                // nearest one (`Capabilities::Charge` in
                // `Billing::Capability` is
                // `Billing::Capabilities::Charge`).
                let resolved = match ctx.lexical_self.as_ref().or(ctx.self_ty.as_ref()) {
                    Some(Ty::Class { id: scope, .. }) => target_ty.map_class_ids(&|id| {
                        lexical_class(id, scope.0.as_str(), self.classes).unwrap_or_else(|| id.clone())
                    }),
                _ => target_ty.clone(),
                };
                let missing = std::cell::RefCell::new(Vec::new());
                // Lowerers also synthesize casts whose identities were proven
                // against the full app before a reduced view/runtime registry
                // retypes them. Resolve source declarations at their boundary.
                if expr.decisions & crate::expr::SOURCE_TYPE_ASCRIPTION != 0 {
                    resolved.map_class_ids(&|id| {
                        if !self.classes.contains_key(id) && !RUBY_TOP_LEVEL.contains(&id.0.as_str())
                            && !self.const_resolver.as_ref().is_some_and(|r| r.has_source_namespace(id.0.as_str())) {
                            missing.borrow_mut().push(id.0.as_str().to_owned());
                        }
                        id.clone()
                    });
                }
                let missing = missing.into_inner();
                if !missing.is_empty() {
                    expr.set_diagnostic(Some(crate::diagnostic::DiagnosticKind::Unsupported {
                        target: None,
                        construct: Symbol::from("declared type"),
                        detail: format!("{} has no indexed declaration or modeled dependency", missing.join(", ")),
                    }));
                } else if matches!(&expr.diagnostic(), Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. }) if construct.as_str() == "declared type") {
                    expr.set_diagnostic(None);
                }
                resolved
            }
        }
    }

}

/// Conditional writes, block writes and parameter rebinding invalidate identity.
fn forget_class_object_writes(expr: &Expr, ctx: &mut Ctx) {
    let mut names = Vec::new();
    rebound_names(expr, &mut names);
    for name in names { ctx.class_objects.remove(&name); ctx.query_class_objects.remove(&name); }
    expr.node.for_each_child(&mut |child| forget_class_object_writes(child, ctx));
}

/// The locals one node binds: assignments, parameters, `let`, pattern
/// and rescue bindings.
fn rebound_names(expr: &Expr, names: &mut Vec<Symbol>) {
    match &*expr.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. }
        | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } => names.push(name.clone()),
        ExprNode::MultiAssign { targets, .. } => {
            for target in targets {
                if let LValue::Var { name, .. } = target { names.push(name.clone()); }
            }
        }
        ExprNode::Lambda { params, rest_param, extra_params, block_param, .. } => {
            names.extend(params.iter().cloned());
            names.extend(rest_param.iter().cloned());
            names.extend(extra_params.iter().map(|p| p.name.clone()));
            names.extend(block_param.iter().cloned());
        }
        ExprNode::Let { name, .. } => names.push(name.clone()),
        ExprNode::CaseMatch { arms, .. } => {
            for arm in arms { arm.pattern.bound_names(names); }
        }
        ExprNode::MatchRequired { pattern, .. } | ExprNode::MatchPredicate { pattern, .. } => pattern.bound_names(names),
        ExprNode::BeginRescue { rescues, .. } => {
            for rescue in rescues { names.extend(rescue.binding.iter().cloned()); }
        }
        _ => {}
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
            Ty::Class { id: ClassId(Symbol::from("Regexp")), args: vec![].into() }
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

/// What `*value` adds to an array literal's elements: an array's or a
/// tuple's elements, a hash's pairs, nothing for `nil`. Ruby `to_a`s any
/// other value; one with no modeled element is kept as the one element
/// `[*object]` gives for an object that is not a collection.
fn splat_elements(ty: Ty) -> Option<Ty> {
    match ty {
        Ty::Nil => None,
        Ty::Tuple { elems } => elems.into_iter().reduce(union_of),
        Ty::Hash { key, value } => Some(Ty::Tuple {
            elems: vec![
                std::sync::Arc::unwrap_or_clone(key),
                std::sync::Arc::unwrap_or_clone(value),
            ]
            .into(),
        }),
        Ty::Union { variants } => variants.into_iter().filter_map(splat_elements).reduce(union_of),
        other => Some(other.collection_elem().unwrap_or(other)),
    }
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

/// The element type of `Kernel#Array(arg)`: an Array stays itself, nil
/// is empty, anything else is wrapped. None when the argument is not
/// known well enough to say (a Hash becomes pairs; not modeled).
fn kernel_array_elem(arg: &Ty) -> Option<Ty> {
    match arg {
        Ty::Array { elem } => Some((**elem).clone()),
        Ty::Tuple { elems } => Some(elems.iter().cloned().reduce(union_of).unwrap_or(Ty::Bottom)),
        // `to_a` of a relation is its records; of a range, its elements.
        Ty::Relation { of } => Some(Ty::Class { id: of.clone(), args: vec![].into() }),
        Ty::Class { id, args } if id.0.as_str() == "Range" => args.first().cloned(),
        Ty::Nil => Some(Ty::Bottom),
        Ty::Union { variants } => variants
            .iter()
            .map(kernel_array_elem)
            .collect::<Option<Vec<_>>>()
            .map(|elems| elems.into_iter().reduce(union_of).unwrap_or(Ty::Bottom)),
        // Any other class may answer `to_ary`/`to_a` (a Struct, a Set,
        // anything Enumerable), which `Array()` then unpacks.
        Ty::Var { .. } | Ty::Untyped | Ty::Hash { .. } | Ty::Record { .. } | Ty::Class { .. } => None,
        scalar => Some(scalar.clone()),
    }
}

pub(super) fn unknown() -> Ty {
    Ty::Var { var: TyVar(0) }
}

fn lambda_call_evidence(
    exprs: &[Expr],
    stmt: usize,
    lambdas: &HashMap<Symbol, lambda_local::Stored>,
    name: &Symbol,
) -> Vec<lambda_local::Evidence> {
    let mut evidence = lambda_local::calls_in(&exprs[stmt], name);
    for (other, stored) in lambdas {
        if other == name || stored.index == stmt {
            continue;
        }
        evidence.extend(lambda_local::calls_in(&exprs[stored.index], name));
    }
    evidence
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

/// Stamp a method's declared `Hash[K, V]` / `Array[E]` return onto
/// bare empty `{}` / `[]` literals on every value-producing return
/// path (sequence tail, `If`/`Case` arms, `return {}`) whose typer-set
/// type is still uninformative (`Var`/`Untyped`).
///
/// Mirrors the Assign-side [`propagate_expected_to_empty_container`]:
/// without this, `def default_url_options; {}; end` typed
/// `-> Hash[Symbol, String]` keeps `Hash<Var, Var>` on the literal,
/// and Crystal emit's empty-hash default (`{} of String => String`)
/// clashes with the annotated return. Call before body-typing when
/// the RBS/`signature` return is known (see `runtime_src`).
pub fn stamp_expected_on_empty_return_tail(body: &mut Expr, expected: &Ty) {
    let peeled = expected.peel_nilable();
    if !matches!(peeled, Ty::Hash { .. } | Ty::Array { .. }) {
        return;
    }
    stamp_empty_return_tail(body, peeled);
}

/// Value-position walk: stamp empty containers that are themselves
/// the method result, and recurse into control-flow that yields a
/// value. Non-tail statements only contribute via explicit `return`.
fn stamp_empty_return_tail(expr: &mut Expr, expected: &Ty) {
    match &mut *expr.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let last = exprs.len() - 1;
            for (i, e) in exprs.iter_mut().enumerate() {
                if i == last {
                    stamp_empty_return_tail(e, expected);
                } else {
                    stamp_returns_in_stmts(e, expected);
                }
            }
        }
        ExprNode::Return { value } => stamp_empty_return_tail(value, expected),
        ExprNode::If {
            then_branch,
            else_branch,
            ..
        } => {
            stamp_empty_return_tail(then_branch, expected);
            stamp_empty_return_tail(else_branch, expected);
        }
        ExprNode::Case { arms, .. } => {
            for arm in arms.iter_mut() {
                stamp_empty_return_tail(&mut arm.body, expected);
            }
        }
        ExprNode::CaseMatch {
            arms, else_body, ..
        } => {
            for arm in arms.iter_mut() {
                stamp_empty_return_tail(&mut arm.body, expected);
            }
            if let Some(eb) = else_body {
                stamp_empty_return_tail(eb, expected);
            }
        }
        ExprNode::BeginRescue {
            body,
            rescues,
            else_branch,
            ensure,
            ..
        } => {
            stamp_empty_return_tail(body, expected);
            for clause in rescues.iter_mut() {
                stamp_empty_return_tail(&mut clause.body, expected);
            }
            if let Some(eb) = else_branch {
                stamp_empty_return_tail(eb, expected);
            }
            if let Some(ens) = ensure {
                stamp_returns_in_stmts(ens, expected);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            stamp_empty_return_tail(expr, expected);
            stamp_empty_return_tail(fallback, expected);
        }
        // Loops / attached blocks are not value tails, but `return {}`
        // inside them exits the enclosing method (Ruby) — except
        // `lambda` / `define_method` blocks (same carve-out as
        // `collect_return_types`).
        ExprNode::While { body, .. } => stamp_returns_in_stmts(body, expected),
        ExprNode::Send {
            method,
            block: Some(block),
            ..
        } if !matches!(method.as_str(), "lambda" | "define_method") => {
            stamp_returns_in_attached_block(block, expected)
        }
        ExprNode::Hash { entries, .. } if entries.is_empty() => {
            if empty_container_ty_uninformative(expr) {
                expr.ty = Some(expected.clone());
            }
        }
        ExprNode::Array { elements, .. } if elements.is_empty() => {
            if empty_container_ty_uninformative(expr) {
                expr.ty = Some(expected.clone());
            }
        }
        _ => {}
    }
}

/// Statement-position walk: only chase explicit `return` (and nested
/// control flow that may contain one). A bare `{}` mid-sequence is not
/// a return value and must not be stamped.
fn stamp_returns_in_stmts(expr: &mut Expr, expected: &Ty) {
    match &mut *expr.node {
        ExprNode::Seq { exprs } => {
            for e in exprs.iter_mut() {
                stamp_returns_in_stmts(e, expected);
            }
        }
        ExprNode::Return { value } => stamp_empty_return_tail(value, expected),
        ExprNode::If {
            then_branch,
            else_branch,
            ..
        } => {
            stamp_returns_in_stmts(then_branch, expected);
            stamp_returns_in_stmts(else_branch, expected);
        }
        ExprNode::Case { arms, .. } => {
            for arm in arms.iter_mut() {
                stamp_returns_in_stmts(&mut arm.body, expected);
            }
        }
        ExprNode::CaseMatch {
            arms, else_body, ..
        } => {
            for arm in arms.iter_mut() {
                stamp_returns_in_stmts(&mut arm.body, expected);
            }
            if let Some(eb) = else_body {
                stamp_returns_in_stmts(eb, expected);
            }
        }
        ExprNode::BeginRescue {
            body,
            rescues,
            else_branch,
            ensure,
            ..
        } => {
            stamp_returns_in_stmts(body, expected);
            for clause in rescues.iter_mut() {
                stamp_returns_in_stmts(&mut clause.body, expected);
            }
            if let Some(eb) = else_branch {
                stamp_returns_in_stmts(eb, expected);
            }
            if let Some(ens) = ensure {
                stamp_returns_in_stmts(ens, expected);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            stamp_returns_in_stmts(expr, expected);
            stamp_returns_in_stmts(fallback, expected);
        }
        ExprNode::While { body, .. } => stamp_returns_in_stmts(body, expected),
        ExprNode::Send {
            method,
            block: Some(block),
            ..
        } if !matches!(method.as_str(), "lambda" | "define_method") => {
            stamp_returns_in_attached_block(block, expected)
        }
        _ => {}
    }
}

/// Chase `return` inside a method-attached block (`Send.block` holding
/// a `Lambda`). Caller must already skip `lambda` / `define_method`.
/// Do not walk lambda literals passed as ordinary args — their `return`
/// exits the lambda, not the enclosing method.
fn stamp_returns_in_attached_block(block: &mut Expr, expected: &Ty) {
    if let ExprNode::Lambda { body, .. } = &mut *block.node {
        stamp_returns_in_stmts(body, expected);
    }
}

/// True when an empty container's `e.ty` carries no emit-usable shape
/// (absent, `Untyped`, `Var`, or `Hash`/`Array` whose params are open).
fn empty_container_ty_uninformative(e: &Expr) -> bool {
    match e.ty.as_ref() {
        None => true,
        Some(Ty::Untyped) | Some(Ty::Var { .. }) => true,
        Some(Ty::Hash { key, value }) => key.is_unknown() && value.is_unknown(),
        Some(Ty::Array { elem }) => elem.is_unknown(),
        _ => false,
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
        Some(Ty::Relation { of }) => Some(Ty::Class { id: of.clone(), args: vec![].into() }),
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
/// Whether statement `i` is not itself one of the nil seeds (only a later
/// statement's writes can have been typed against the seed).
fn si_is_not(i: usize, nil_seed: &HashMap<Symbol, (usize, Ty)>) -> bool {
    nil_seed.values().all(|(si, _)| *si != i)
}

fn collect_local_assignment_tys(expr: &Expr, out: &mut Vec<(Symbol, Ty)>) {
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*expr.node {
        if let Some(t) = value.ty.clone() {
            if !t.is_open() {
                out.push((name.clone(), t));
            }
        }
    }
    // `x ||= v` writes v where x was nil, the seed's own case (lobsters' `top_comment_depth ||= comment.depth`).
    if let ExprNode::OpAssign { target: LValue::Var { name, .. }, op: crate::expr::OpAssignOp::OrOr, value } = &*expr.node {
        if let Some(t) = value.ty.clone() {
            if !t.is_open() {
                out.push((name.clone(), t));
            }
        }
    }
    expr.node.for_each_child(&mut |child| collect_local_assignment_tys(child, out));
}

/// Walk one statement's control flow forward, recording in `flowed` the
/// type each local holds where the statement completes. `bound` is what
/// the locals held before it; `flowed` carries only the names written
/// since. Where paths join (`if`/`case` arms, a loop that may not run, the
/// right side of `&&`/`||`, a `rescue` that may fire before the body's
/// assignments), a name takes the union of every arriving path, with nil
/// for a path on which it was never bound. Returns false when no path
/// completes (`return`, `raise`, `next`, …); such a path joins nothing.
///
/// A write this walk does not model (inside a block, a pattern, a
/// destructuring, or with a still-open type) puts the name in `unknown`,
/// and the caller exports nothing for it: a join that missed one write
/// would be a confident wrong type, worse than none.
fn flow_locals(
    expr: &Expr,
    bound: &HashMap<Symbol, Ty>,
    flowed: &mut HashMap<Symbol, Ty>,
    unknown: &mut crate::det::HashSet<Symbol>,
) -> bool {
    // Each path is its writes plus what its condition narrowed. A
    // narrowing stands in for the entry type on a path that does not
    // write the name (`child = {} unless child.is_a?(Hash)` leaves a
    // Hash either way), but only the writes decide which names join.
    type Path = (HashMap<Symbol, Ty>, HashMap<Symbol, Ty>);
    fn join(paths: Vec<Path>, bound: &HashMap<Symbol, Ty>) -> Option<HashMap<Symbol, Ty>> {
        let mut names: Vec<&Symbol> = paths.iter().flat_map(|(w, _)| w.keys()).collect();
        names.sort();
        names.dedup();
        let mut out = HashMap::default();
        for name in names {
            let mut tys = paths.iter().map(|(writes, narrowed)| {
                writes.get(name).or_else(|| narrowed.get(name)).or_else(|| bound.get(name))
                    .cloned().unwrap_or(Ty::Nil)
            });
            let first = tys.next()?;
            out.insert(name.clone(), tys.fold(first, union_of));
        }
        (!paths.is_empty()).then_some(out)
    }
    fn unmodeled(expr: &Expr, unknown: &mut crate::det::HashSet<Symbol>) {
        let mut names = Vec::new();
        match &*expr.node {
            ExprNode::Assign { target: LValue::Var { name, .. }, .. }
            | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } => names.push(name.clone()),
            ExprNode::MultiAssign { targets, .. } => {
                for target in targets {
                    if let LValue::Var { name, .. } = target { names.push(name.clone()); }
                }
            }
            ExprNode::CaseMatch { arms, .. } => {
                for arm in arms { arm.pattern.bound_names(&mut names); }
            }
            ExprNode::MatchRequired { pattern, .. } | ExprNode::MatchPredicate { pattern, .. } => {
                pattern.bound_names(&mut names)
            }
            ExprNode::BeginRescue { rescues, .. } => {
                for rescue in rescues { names.extend(rescue.binding.iter().cloned()); }
            }
            _ => {}
        }
        unknown.extend(names);
        expr.node.for_each_child(&mut |child| unmodeled(child, unknown));
    }
    let flow = |e: &Expr, flowed: &mut HashMap<Symbol, Ty>, unknown: &mut _| flow_locals(e, bound, flowed, unknown);
    match &*expr.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            if !flow(value, flowed, unknown) { return false; }
            match value.ty.clone() {
                Some(t) if !t.is_open() => { flowed.insert(name.clone(), t); }
                _ => { unknown.insert(name.clone()); }
            }
            true
        }
        // The Seq arm's `||=` threading: union with what the name held.
        ExprNode::OpAssign { target: LValue::Var { name, .. }, value, .. } => {
            if !flow(value, flowed, unknown) { return false; }
            match expr.ty.clone() {
                Some(t) if !t.is_open() => {
                    let merged = match flowed.get(name).or_else(|| bound.get(name)) {
                        Some(prev) => union_of(prev.clone(), t),
                        None => t,
                    };
                    flowed.insert(name.clone(), merged);
                }
                _ => { unknown.insert(name.clone()); }
            }
            true
        }
        ExprNode::Seq { exprs } => exprs.iter().all(|e| flow(e, flowed, unknown)),
        ExprNode::If { cond, then_branch, else_branch } => {
            if !flow(cond, flowed, unknown) { return false; }
            let pred = narrowing::extract_narrowing_with(cond, &narrowing::Probe::without_members(&|_| None));
            let mut paths = Vec::new();
            for (branch, then_side) in [(then_branch, true), (else_branch, false)] {
                let mut narrowed = match &pred {
                    Some(p) => narrowing::narrow_locals(p, then_side, &|name| {
                        flowed.get(name).or_else(|| bound.get(name)).cloned()
                    }),
                    None => HashMap::default(),
                };
                // A name written before the branch (in the condition
                // itself: `return if !(story = find_story)`) holds what
                // the condition proved about that write on this side.
                let mut path = flowed.clone();
                narrowed.retain(|name, ty| match path.get_mut(name) {
                    Some(written) => { *written = ty.clone(); false }
                    None => true,
                });
                if flow(branch, &mut path, unknown) { paths.push((path, narrowed)); }
            }
            match join(paths, bound) {
                Some(joined) => { *flowed = joined; true }
                None => false,
            }
        }
        ExprNode::Case { scrutinee, arms } => {
            if !flow(scrutinee, flowed, unknown) { return false; }
            let mut paths = Vec::new();
            for arm in arms {
                let mut path = flowed.clone();
                let completes = arm.guard.as_ref().is_none_or(|g| flow(g, &mut path, unknown))
                    && flow(&arm.body, &mut path, unknown);
                if completes { paths.push((path, HashMap::default())); }
            }
            // No catch-all arm: no arm may match, and the value is nil.
            let exhaustive = arms.iter().any(|a| {
                matches!(a.pattern, crate::expr::Pattern::Wildcard) && a.guard.is_none()
            });
            if !exhaustive { paths.push((flowed.clone(), HashMap::default())); }
            match join(paths, bound) {
                Some(joined) => { *flowed = joined; true }
                None => false,
            }
        }
        // The body may run zero times; `break`/`next` leave it with the
        // writes made so far, so its end state joins whether or not it
        // completes.
        ExprNode::While { cond, body, .. } => {
            if !flow(cond, flowed, unknown) { return false; }
            let mut path = flowed.clone();
            flow(body, &mut path, unknown);
            *flowed = join(vec![(flowed.clone(), HashMap::default()), (path, HashMap::default())], bound).unwrap_or_default();
            true
        }
        ExprNode::BoolOp { left, right, .. } => {
            if !flow(left, flowed, unknown) { return false; }
            let mut path = flowed.clone();
            let mut paths = vec![(flowed.clone(), HashMap::default())];
            if flow(right, &mut path, unknown) { paths.push((path, HashMap::default())); }
            *flowed = join(paths, bound).unwrap_or_default();
            true
        }
        // A rescue can fire before or after any of the body's writes, so
        // it starts from the join of the entry and the body's end state.
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            let mut body_path = flowed.clone();
            let mut completes = flow(body, &mut body_path, unknown);
            if completes {
                if let Some(eb) = else_branch {
                    completes = flow(eb, &mut body_path, unknown);
                }
            }
            let entry = join(vec![(flowed.clone(), HashMap::default()), (body_path.clone(), HashMap::default())], bound)
                .unwrap_or_default();
            let mut paths = Vec::new();
            if completes { paths.push((body_path, HashMap::default())); }
            for rescue in rescues {
                unknown.extend(rescue.binding.iter().cloned());
                let mut path = entry.clone();
                if flow(&rescue.body, &mut path, unknown) { paths.push((path, HashMap::default())); }
            }
            let joined = join(paths, bound);
            if let Some(ensure) = ensure {
                let mut path = joined.clone().unwrap_or_else(|| entry.clone());
                if !flow(ensure, &mut path, unknown) { return false; }
                if joined.is_some() { *flowed = path; return true; }
                return false;
            }
            match joined {
                Some(joined) => { *flowed = joined; true }
                None => false,
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                if !flow(r, flowed, unknown) { return false; }
            }
            for a in args {
                if !flow(a, flowed, unknown) { return false; }
            }
            if let Some(b) = block { unmodeled(b, unknown); }
            true
        }
        ExprNode::Return { value } | ExprNode::Raise { value } => {
            flow(value, flowed, unknown);
            false
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value { flow(v, flowed, unknown); }
            false
        }
        ExprNode::Retry | ExprNode::Redo => false,
        _ => {
            unmodeled(expr, unknown);
            true
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
        // `==` ignores record field order; keep the canonical one.
        return Ty::canonical_min(a, b);
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
                key: std::sync::Arc::new(union_of((**k1).clone(), (**k2).clone())),
                value: std::sync::Arc::new(union_of((**v1).clone(), (**v2).clone())),
            };
        }
        (Ty::Array { elem: e1 }, Ty::Array { elem: e2 }) => {
            return Ty::Array {
                elem: std::sync::Arc::new(union_of((**e1).clone(), (**e2).clone())),
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
    // Joining a large canonical union with variants it already holds is
    // the fixpoint's common case (a parameter's stored union met by each
    // further call site); the rebuild below would be quadratic in its
    // size for no change. Variant order is untouched: the result is `a`.
    if union_contains_all(&a, &b) {
        return a;
    }
    let mut acc = UnionAcc::default();
    push_union_variants(a, &mut acc);
    push_union_variants(b, &mut acc);
    let mut variants = acc.out;
    Ty::canonicalize_variants(&mut variants);
    match variants.len() {
        0 => Ty::Bottom,
        1 => variants.into_iter().next().unwrap(),
        _ => Ty::Union { variants: variants.into() },
    }
}

/// Unions at or above this many variants find an equal variant through a
/// fingerprint index instead of scanning with `==`.
const UNION_INDEX_MIN: usize = 24;

/// True when `a` is a large union already in canonical form and every
/// variant of `b` is one it holds, so joining them returns `a` unchanged
/// (`union_of`'s flatten/dedup/sort would rebuild the same list).
fn union_contains_all(a: &Ty, b: &Ty) -> bool {
    let Ty::Union { variants: av } = a else { return false };
    if av.len() < UNION_INDEX_MIN {
        return false;
    }
    let bs = match b {
        Ty::Union { variants } => &variants[..],
        other => std::slice::from_ref(other),
    };
    bs.iter().all(|v| {
        !matches!(v, Ty::Union { .. }) && !v.holds_record() && Ty::find_in_sorted_variants(av, v).is_some()
    }) && Ty::is_canonical_union_without_records(av)
}

/// The variant list `push_union_variants` builds, with a lazily built
/// fingerprint index over the non-container variants once it is large.
#[derive(Default)]
struct UnionAcc {
    out: Vec<Ty>,
    /// Fingerprint → positions in `out`, covering `out[..indexed]`.
    index: HashMap<u64, Vec<usize>>,
    indexed: usize,
}

impl UnionAcc {
    /// The first position holding a variant `==` to `other`. `other` is
    /// neither `Hash` nor `Array`, so those entries (mutated in place by
    /// the merge arms) never match and are left out of the index.
    fn position_of(&mut self, other: &Ty) -> Option<usize> {
        if self.out.len() < UNION_INDEX_MIN {
            return self.out.iter().position(|v| v == other);
        }
        while self.indexed < self.out.len() {
            let v = &self.out[self.indexed];
            if !matches!(v, Ty::Hash { .. } | Ty::Array { .. }) {
                self.index.entry(v.eq_fingerprint()).or_default().push(self.indexed);
            }
            self.indexed += 1;
        }
        self.index.get(&other.eq_fingerprint())?.iter().copied().find(|&i| self.out[i] == *other)
    }
}

/// Append `t`'s constituent variants to `acc`, flattening nested
/// `Union`s and skipping any variant already present (structural
/// equality) so the result carries each distinct type once. A `Hash`
/// or `Array` merges pointwise into an existing same-spine variant
/// instead of appending, keeping the union to one spine per container
/// kind regardless of the order joins arrive in.
fn push_union_variants(t: Ty, acc: &mut UnionAcc) {
    match t {
        Ty::Union { variants } => {
            for v in variants {
                push_union_variants(v, acc);
            }
        }
        Ty::Hash { key, value } => {
            for existing in acc.out.iter_mut() {
                if let Ty::Hash { key: k0, value: v0 } = existing {
                    *k0 = std::sync::Arc::new(union_of((**k0).clone(), std::sync::Arc::unwrap_or_clone(key)));
                    *v0 = std::sync::Arc::new(union_of((**v0).clone(), std::sync::Arc::unwrap_or_clone(value)));
                    return;
                }
            }
            acc.out.push(Ty::Hash { key, value });
        }
        Ty::Array { elem } => {
            for existing in acc.out.iter_mut() {
                if let Ty::Array { elem: e0 } = existing {
                    *e0 = std::sync::Arc::new(union_of((**e0).clone(), std::sync::Arc::unwrap_or_clone(elem)));
                    return;
                }
            }
            acc.out.push(Ty::Array { elem });
        }
        other => match acc.position_of(&other) {
            // An equal variant can differ in record field order; keep
            // the canonical one so the result doesn't depend on which
            // arrived first.
            Some(i) => {
                let existing = &mut acc.out[i];
                *existing = Ty::canonical_min(existing.clone(), other);
            }
            None => acc.out.push(other),
        },
    }
}

/// The join for an ivar slot the fixpoint carries between rounds
/// (`extract_ivar_assignments`' repeated writes and `[]=` widening).
///
/// [`union_of`] with a pending `Var` as the identity: a `Var` arm
/// drops out wherever another arm sits beside it, at the top and
/// inside Hash and Array spines, so `{}` (`Hash[Var, Var]`) joined
/// with a `[]=` write gives the same Hash whichever arrived first, as
/// `widen_hash_ivar_value` always meant (it replaced a `Var` value
/// side). `Untyped` stays an arm: gradual absorption is unchanged
/// (#617).
pub(crate) fn join_ivar_slot(a: Ty, b: Ty) -> Ty {
    drop_pending_arms(union_of(a, b))
}

/// Remove `Var` arms that sit beside a non-`Var` arm, in a union and
/// inside Hash and Array spines. A union of `Var`s alone keeps the one
/// with the smallest id (canonical order sorts them first), so every
/// pending value is one element of the lattice. Expects `union_of`'s
/// canonical form and returns it.
pub(crate) fn drop_pending_arms(t: Ty) -> Ty {
    match t {
        Ty::Hash { key, value } => Ty::Hash {
            key: std::sync::Arc::new(drop_pending_arms(std::sync::Arc::unwrap_or_clone(key))),
            value: std::sync::Arc::new(drop_pending_arms(std::sync::Arc::unwrap_or_clone(value))),
        },
        Ty::Array { elem } => Ty::Array { elem: std::sync::Arc::new(drop_pending_arms(std::sync::Arc::unwrap_or_clone(elem))) },
        Ty::Union { variants } if variants.len() >= UNION_INDEX_MIN && is_settled_arm_list(&variants) => {
            Ty::Union { variants }
        }
        Ty::Union { variants } => {
            let mut variants: Vec<Ty> = variants.into_iter().map(drop_pending_arms).collect();
            if variants.iter().any(|v| !matches!(v, Ty::Var { .. })) {
                variants.retain(|v| !matches!(v, Ty::Var { .. }));
            } else {
                variants.truncate(1);
            }
            Ty::canonicalize_variants(&mut variants);
            match variants.len() {
                1 => variants.pop().unwrap(),
                _ => Ty::Union { variants: variants.into() },
            }
        }
        other => other,
    }
}

/// Whether `drop_pending_arms` would return this arm list as it stands:
/// no `Var` or nested container to rewrite, and already in sorted order
/// (so its re-sort is a no-op).
fn is_settled_arm_list(variants: &[Ty]) -> bool {
    variants.iter().all(|v| !matches!(v, Ty::Var { .. } | Ty::Union { .. } | Ty::Hash { .. } | Ty::Array { .. }))
        && variants.windows(2).all(|w| Ty::cmp_nil_last(&w[0], &w[1]) != std::cmp::Ordering::Greater)
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
        HashMap::default()
    }

    fn ctx_with_local(name: &str, ty: Ty) -> Ctx {
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(Symbol::from(name), ty);
        ctx
    }

    #[test]
    fn const_class_ref_keeps_class_object_without_registry_entry() {
        // Lower retypes with a partial registry that omits stdlib. A Const
        // whose type is the written class must still stamp CLASS_OBJECT_VALUE
        // so `RecordNotFound < StandardError` classifies as Module#<.
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = Ctx::default();
        let mut expr = synth(ExprNode::Const {
            path: vec![Symbol::from("StandardError")],
        });
        let ty = typer.analyze_expr(&mut expr, &ctx);
        assert_eq!(
            ty,
            Ty::Class {
                id: ClassId(Symbol::from("StandardError")),
                args: vec![].into(),
            }
        );
        assert_ne!(
            expr.decisions & crate::expr::CLASS_OBJECT_VALUE,
            0,
            "Const class ref must remain a class object without a registry entry"
        );
    }

    fn optional_str() -> Ty {
        Ty::Union {
            variants: vec![Ty::Str, Ty::Nil].into(),
        }
    }


    #[test]
    fn kernel_array_does_not_capture_inherited_or_included_app_methods() {
        let owner = ClassId(Symbol::from("Owner"));
        let child = ClassId(Symbol::from("Child"));
        for (included, ret) in [(false, Ty::Str), (true, Ty::Str), (false, Ty::Untyped)] {
            let mut classes = empty_classes();
            let mut info = ClassInfo::default();
            info.instance_methods.insert(Symbol::from("Array"), ret.clone());
            classes.insert(owner.clone(), info);
            let mut info = ClassInfo::default();
            if included {
                info.includes.push(owner.clone());
            } else {
                info.parent = Some(owner.clone());
            }
            classes.insert(child.clone(), info);
            let mut ctx = Ctx::default();
            ctx.self_ty = Some(Ty::Class { id: child.clone(), args: vec![].into() });
            let mut expr = send(None, "Array", vec![nil_lit()]);
            assert_eq!(BodyTyper::new(&classes).analyze_expr(&mut expr, &ctx), ret);
        }
    }

    #[test]
    fn kernel_array_keeps_the_argument_element_type() {
        let array_of = |elem: Ty| Ty::Array { elem: std::sync::Arc::new(elem) };
        for (arg, elem) in [
            (array_of(Ty::Str), Ty::Str),
            (Ty::Tuple { elems: vec![Ty::Str, Ty::Int].into() }, union_of(Ty::Str, Ty::Int)),
            (Ty::Relation { of: ClassId(Symbol::from("Story")) }, Ty::Class { id: ClassId(Symbol::from("Story")), args: vec![].into() }),
            (Ty::Class { id: ClassId(Symbol::from("Range")), args: vec![Ty::Int].into() }, Ty::Int),
            // Unknown elements: the class may unpack through `to_a`.
            (Ty::Class { id: ClassId(Symbol::from("Story")), args: vec![].into() }, Ty::Var { var: TyVar(0) }),
            (Ty::Sym, Ty::Sym),
            (Ty::Union { variants: vec![array_of(Ty::Sym), Ty::Sym, Ty::Nil].into() }, Ty::Sym),
        ] {
            let ctx = ctx_with_local("x", arg);
            let mut expr = send(None, "Array", vec![var("x")]);
            assert_eq!(BodyTyper::new(&empty_classes()).analyze_expr(&mut expr, &ctx), array_of(elem));
        }
    }

    #[test]
    fn array_plus_onto_an_unknown_element_takes_the_argument_element() {
        let array_of = |elem: Ty| Ty::Array { elem: std::sync::Arc::new(elem) };
        let mut ctx = ctx_with_local("empty", array_of(Ty::Var { var: TyVar(0) }));
        ctx.local_bindings.insert(Symbol::from("strs"), array_of(Ty::Str));
        ctx.local_bindings.insert(Symbol::from("syms"), array_of(Ty::Sym));
        let mut expr = send(Some(var("empty")), "+", vec![var("strs")]);
        assert_eq!(BodyTyper::new(&empty_classes()).analyze_expr(&mut expr, &ctx), array_of(Ty::Str));
        // A known receiver element still answers for the result.
        let mut expr = send(Some(var("strs")), "+", vec![var("syms")]);
        assert_eq!(BodyTyper::new(&empty_classes()).analyze_expr(&mut expr, &ctx), array_of(Ty::Str));
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
            variants: vec![Ty::Str, Ty::Int, Ty::Nil].into(),
        };
        let ctx = ctx_with_local("x", mixed);
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
        assert_eq!(
            else_ctx.local_bindings[&Symbol::from("x")],
            Ty::Union {
                variants: vec![Ty::Int, Ty::Nil].into(),
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
                args: vec![].into(),
            },
        );
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert_eq!(
            then_ctx.local_bindings[&Symbol::from("x")],
            Ty::Class {
                id: ClassId(Symbol::from("Post")),
                args: vec![].into()
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
        synth(ExprNode::Lambda { extra_params: Vec::new(), rest_param: None,
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
            e.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
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
            Ty::Array { elem: std::sync::Arc::new(Ty::Int) },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
    }

    #[test]
    fn an_array_literal_holds_a_splatted_arrays_elements() {
        // [*arr, "x"] on arr: Array[Array[Str]] is Array[Array[Str] | Str],
        // and a splatted nil adds nothing.
        let splat = |name: &str| synth(ExprNode::Splat { value: var(name) });
        let mut expr = synth(ExprNode::Array {
            elements: vec![splat("arr"), splat("none"), synth(ExprNode::Lit { value: Literal::Str { value: "x".into() } })],
            style: Default::default(),
        });
        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let mut ctx = Ctx::default();
        let inner = Ty::Array { elem: std::sync::Arc::new(Ty::Str) };
        ctx.local_bindings.insert(Symbol::from("arr"), Ty::Array { elem: std::sync::Arc::new(inner.clone()) });
        ctx.local_bindings.insert(Symbol::from("none"), Ty::Nil);
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: std::sync::Arc::new(union_of(inner, Ty::Str)) });
    }

    #[test]
    fn array_select_preserves_element_type() {
        // arr.select { |x| x > 0 } on arr: Array[Int] should still be Array[Int]
        let arr = {
            let mut e = var("arr");
            e.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
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
            Ty::Array { elem: std::sync::Arc::new(Ty::Int) },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
    }

    #[test]
    fn array_flat_map_flattens_one_level() {
        // arr.flat_map { |x| [x.to_s] } on arr: Array[Int] should be Array[Str]
        let arr = {
            let mut e = var("arr");
            e.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
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
            Ty::Array { elem: std::sync::Arc::new(Ty::Int) },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
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
        assert_eq!(ty, Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
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

        let expected = Ty::Array { elem: std::sync::Arc::new(Ty::Int) };
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
                key: std::sync::Arc::new(Ty::Str),
                value: std::sync::Arc::new(Ty::Untyped),
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
                key: std::sync::Arc::new(Ty::Sym),
                value: std::sync::Arc::new(Ty::Int),
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
                key: std::sync::Arc::new(Ty::Sym),
                value: std::sync::Arc::new(Ty::Int),
            },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: std::sync::Arc::new(Ty::Str) });
    }

    #[test]
    fn hash_transform_values_changes_value_type() {
        // h.transform_values { |v| v.to_s } on Hash[Sym, Int] → Hash[Sym, Str]
        let h = {
            let mut e = var("h");
            e.ty = Some(Ty::Hash {
                key: std::sync::Arc::new(Ty::Sym),
                value: std::sync::Arc::new(Ty::Int),
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
                key: std::sync::Arc::new(Ty::Sym),
                value: std::sync::Arc::new(Ty::Int),
            },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(
            ty,
            Ty::Hash {
                key: std::sync::Arc::new(Ty::Sym),
                value: std::sync::Arc::new(Ty::Str),
            }
        );
    }

    #[test]
    fn map_without_block_falls_back_to_input_elem() {
        // arr.map (no block — Symbol-to-Proc pattern handled elsewhere)
        // should still produce a sensible Array[elem] type.
        let arr = {
            let mut e = var("arr");
            e.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
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
            Ty::Array { elem: std::sync::Arc::new(Ty::Int) },
        );
        let ty = typer.analyze_expr(&mut expr, &ctx);

        assert_eq!(ty, Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
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

        let diag = expr.diagnostic().as_ref().expect("diagnostic set");
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

        let diag = expr.diagnostic().as_ref().expect("diagnostic set");
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

        assert!(expr.diagnostic().is_none());
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

        let diag = expr.diagnostic().as_ref().expect("diagnostic set");
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

        assert!(expr.diagnostic().is_none());
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
                key: std::sync::Arc::new(Ty::Sym),
                value: std::sync::Arc::new(Ty::Int),
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

        let diag = expr.diagnostic().as_ref().expect("diagnostic set");
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

        let diag = expr.diagnostic().as_ref().expect("diagnostic set");
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

        assert!(expr.diagnostic().is_none());
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
            args: vec![].into(),
        };
        let arr = |elem: Ty| Ty::Array { elem: std::sync::Arc::new(elem) };
        let hash = |k: Ty, v: Ty| Ty::Hash {
            key: std::sync::Arc::new(k),
            value: std::sync::Arc::new(v),
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
            Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() },
            Ty::Union { variants: vec![Ty::Int, Ty::Str].into() },
            Ty::Union {
                variants: vec![hash(Ty::Str, Ty::Int), Ty::Nil].into(),
            },
        ]
    }

    fn class_ty(name: &str) -> Ty {
        Ty::Class { id: crate::ident::ClassId(crate::ident::Symbol::from(name)), args: vec![].into() }
    }

    /// The join as it was before the large-union index: scan for an equal
    /// variant, flatten, sort.
    fn naive_union_of(a: Ty, b: Ty) -> Ty {
        fn push(t: Ty, out: &mut Vec<Ty>) {
            match t {
                Ty::Union { variants } => variants.iter().cloned().for_each(|v| push(v, out)),
                other => match out.iter().position(|v| *v == other) {
                    Some(i) => out[i] = Ty::canonical_min(out[i].clone(), other),
                    None => out.push(other),
                },
            }
        }
        let mut out = Vec::new();
        push(a, &mut out);
        push(b, &mut out);
        Ty::canonicalize_variants(&mut out);
        Ty::Union { variants: out.into() }
    }

    #[test]
    fn large_union_join_keeps_variant_order() {
        // 200 classes joined in a scrambled order, repeatedly: the stored
        // union is the same list whichever order they arrive in, and
        // re-joining members returns it untouched.
        let names: Vec<String> = (0..200).map(|i| format!("Model{:03}", (i * 37) % 200)).collect();
        let mut stored = Ty::Nil;
        for n in &names {
            stored = union_of(stored, class_ty(n));
        }
        let Ty::Union { variants } = &stored else { panic!("{stored:?}") };
        assert_eq!(variants.len(), 201);
        assert!(variants.windows(2).all(|w| Ty::cmp_nil_last(&w[0], &w[1]).is_lt()));
        assert_eq!(variants.last(), Some(&Ty::Nil));
        let mut sorted: Vec<&str> = variants[..200]
            .iter()
            .map(|v| match v { Ty::Class { id, .. } => id.0.as_str(), other => panic!("{other:?}") })
            .collect();
        let before = sorted.clone();
        sorted.sort();
        assert_eq!(sorted, before);
        for n in names.iter().rev() {
            assert_eq!(union_of(stored.clone(), class_ty(n)), stored);
            assert_eq!(union_of(class_ty(n), stored.clone()), stored);
        }
        assert_eq!(union_of(stored.clone(), Ty::Nil), stored);
        // A new variant lands in sorted position, not at the end.
        let joined = union_of(stored.clone(), class_ty("Model0005x"));
        let Ty::Union { variants: v2 } = &joined else { panic!() };
        assert_eq!(v2.len(), 202);
        assert_eq!(joined, naive_union_of(stored.clone(), class_ty("Model0005x")));
        // A union observed against it, partly overlapping.
        let other = Ty::Union { variants: vec![class_ty("Model010"), class_ty("Zed"), Ty::Int].into() };
        assert_eq!(union_of(stored.clone(), other.clone()), naive_union_of(stored.clone(), other));
    }

    #[test]
    fn large_union_join_with_records_and_containers_matches_naive() {
        let rec = |fields: &[(&str, Ty)]| {
            let mut m = indexmap::IndexMap::new();
            for (k, v) in fields {
                m.insert(crate::ident::Symbol::from(*k), v.clone());
            }
            Ty::Record { row: crate::ty::Row { fields: m.into(), rest: None } }
        };
        let mut variants: Vec<Ty> = (0..40).map(|i| class_ty(&format!("C{i:02}"))).collect();
        variants.push(rec(&[("a", Ty::Int), ("b", Ty::Str)]));
        variants.push(Ty::Int);
        let big = naive_union_of(Ty::Union { variants: variants.into() }, Ty::Str);
        for obs in [
            rec(&[("b", Ty::Str), ("a", Ty::Int)]),
            rec(&[("a", Ty::Int), ("b", Ty::Str)]),
            Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(Ty::Int) },
            Ty::Array { elem: std::sync::Arc::new(Ty::Int) },
            class_ty("C07"),
            Ty::Nil,
            Ty::Union { variants: vec![class_ty("C07"), Ty::Int].into() },
        ] {
            assert_eq!(union_of(big.clone(), obs.clone()), naive_union_of(big.clone(), obs.clone()), "{obs:?}");
            let once = union_of(big.clone(), obs.clone());
            assert_eq!(union_of(once.clone(), obs.clone()), once);
            assert_eq!(crate::analyze::unify_param_ty(big.clone(), obs.clone()), drop_pending_arms(union_of(big.clone(), obs.clone())));
        }
    }

    /// Joining members into a large stored union must not be quadratic: a
    /// scan of `==` per variant took seconds at this size, the index and
    /// containment check take milliseconds. Generous bound, so it only
    /// trips on a regression to the scan. Only the re-joins are timed:
    /// growing the union one new variant at a time still re-sorts it per
    /// join, which a debug build takes tens of seconds for at this size.
    #[test]
    fn large_union_join_is_not_quadratic() {
        let n = 4000;
        let classes: Vec<Ty> = (0..n).map(|i| class_ty(&format!("Model{i:05}"))).collect();
        let mut variants = classes.clone();
        Ty::canonicalize_variants(&mut variants);
        let mut stored = Ty::Union { variants: variants.into() };
        let Ty::Union { variants } = &stored else { panic!() };
        assert!(Ty::is_canonical_union_without_records(variants));
        let started = std::time::Instant::now();
        for c in classes.iter().step_by(7) {
            stored = union_of(stored, c.clone());
        }
        let Ty::Union { variants } = &stored else { panic!() };
        assert_eq!(variants.len(), n);
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
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
    fn union_of_bool_int_nil_is_structural_not_debug_order() {
        // Debug-string sort put Bool before Int; `ty_tag` puts Int first.
        let got = union_of(union_of(Ty::Bool, Ty::Int), Ty::Nil);
        assert_eq!(
            got,
            Ty::Union {
                variants: vec![Ty::Int, Ty::Bool, Ty::Nil].into()
            }
        );
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
            key: std::sync::Arc::new(Ty::Str),
            value: std::sync::Arc::new(Ty::Int),
        };
        let h2 = Ty::Hash {
            key: std::sync::Arc::new(Ty::Sym),
            value: std::sync::Arc::new(Ty::Str),
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

    fn record(fields: &[(&str, Ty)]) -> Ty {
        Ty::Record {
            row: Row {
                fields: fields.iter().map(|(k, t)| (Symbol::from(*k), t.clone())).collect(),
                rest: None,
            },
        }
    }

    fn field_order(t: &Ty) -> Vec<String> {
        match t {
            Ty::Record { row } => row.fields.keys().map(|k| k.as_str().to_string()).collect(),
            other => panic!("expected a record, got {other:?}"),
        }
    }

    /// Record rows are equal regardless of field order, so a union's
    /// canonical variant order must not depend on it either.
    #[test]
    fn union_of_orders_records_regardless_of_field_order() {
        let ab_int = record(&[("a", Ty::Int), ("b", Ty::Int)]);
        let ba_int = record(&[("b", Ty::Int), ("a", Ty::Int)]);
        let ab_str = record(&[("a", Ty::Str), ("b", Ty::Int)]);
        assert_eq!(
            union_of(ab_int.clone(), ab_str.clone()),
            union_of(ab_str.clone(), ba_int.clone()),
        );
        for (x, y) in [(ab_int.clone(), ab_str.clone()), (ba_int.clone(), ab_str.clone())] {
            check_record_laws(x, y);
        }
    }

    fn check_record_laws(x: Ty, y: Ty) {
        let universe = [x.clone(), y.clone(), Ty::Nil, Ty::Int, union_of(x, Ty::Nil), union_of(y, Ty::Str)];
        for a in &universe {
            for b in &universe {
                assert_eq!(union_of(a.clone(), b.clone()), union_of(b.clone(), a.clone()), "{a:?} ⊔ {b:?}");
                for c in &universe {
                    assert_eq!(
                        union_of(union_of(a.clone(), b.clone()), c.clone()),
                        union_of(a.clone(), union_of(b.clone(), c.clone())),
                    );
                }
            }
        }
    }

    /// Two arrivals of one record in different field orders keep one
    /// of the source orders, and the same one whichever came first:
    /// some targets emit fields in that order.
    #[test]
    fn union_of_keeps_one_source_field_order_for_equal_records() {
        let ab = record(&[("a", Ty::Int), ("b", Ty::Str)]);
        let ba = record(&[("b", Ty::Str), ("a", Ty::Int)]);
        let one = union_of(ab.clone(), ba.clone());
        let other = union_of(ba.clone(), ab.clone());
        assert_eq!(field_order(&one), field_order(&other));
        let nil_first = union_of(union_of(ba.clone(), Ty::Nil), ab.clone());
        let nil_last = union_of(union_of(ab, Ty::Nil), ba);
        let order = |t: &Ty| match t {
            Ty::Union { variants } => field_order(&variants[0]),
            other => panic!("expected a union, got {other:?}"),
        };
        assert_eq!(order(&nil_first), order(&nil_last));
        assert_eq!(order(&nil_first), field_order(&one));
    }

    // Lattice laws for every join that feeds a slot the fixpoint carries
    // from one round to the next (#617, "Any order"). A worklist that
    // visits producers in another order joins the same observations in
    // another order and grouping, so each carried-slot join has to be
    // commutative, associative and idempotent, with pending (`Var`) as
    // its identity. The universe is generated from `law_universe` plus
    // the unknown-arm shapes the joins classify, normalized through the
    // join under test (a carried slot only ever holds a join's output),
    // and every law is checked over every pair and triple.

    fn pending() -> Ty {
        Ty::Var { var: TyVar(0) }
    }

    fn carried_slot_universe(join: fn(Ty, Ty) -> Ty) -> Vec<Ty> {
        let mut raw = law_universe();
        raw.extend([
            Ty::Var { var: TyVar(2) },
            Ty::Union { variants: vec![Ty::Str, Ty::Untyped].into() },
            Ty::Union { variants: vec![Ty::Int, Ty::Var { var: TyVar(4) }].into() },
            Ty::Union { variants: vec![Ty::Untyped, Ty::Nil].into() },
            Ty::Union { variants: vec![Ty::Var { var: TyVar(5) }, Ty::Nil].into() },
            Ty::Union { variants: vec![Ty::Var { var: TyVar(6) }, Ty::Untyped, Ty::Nil].into() },
            Ty::Union {
                variants: vec![
                    Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(Ty::Untyped) },
                    Ty::Nil,
                ].into(),
            },
        ]);
        let mut out: Vec<Ty> = Vec::new();
        for t in raw {
            let t = join(pending(), t);
            if !out.contains(&t) {
                out.push(t);
            }
        }
        out
    }

    fn check_carried_slot_join_laws(name: &str, join: fn(Ty, Ty) -> Ty) {
        let universe = carried_slot_universe(join);
        for a in &universe {
            assert_eq!(join(pending(), a.clone()), *a, "{name}: pending ⊔ {a:?}");
            assert_eq!(join(a.clone(), pending()), *a, "{name}: {a:?} ⊔ pending");
            assert_eq!(join(a.clone(), a.clone()), *a, "{name}: {a:?} ⊔ itself");
            for b in &universe {
                let ab = join(a.clone(), b.clone());
                assert_eq!(ab, join(b.clone(), a.clone()), "{name}: {a:?} ⊔ {b:?} must commute");
                for c in &universe {
                    assert_eq!(
                        join(ab.clone(), c.clone()),
                        join(a.clone(), join(b.clone(), c.clone())),
                        "{name}: ({a:?} ⊔ {b:?}) ⊔ {c:?} must associate",
                    );
                }
            }
        }
    }

    #[test]
    fn param_slot_join_is_a_lattice_join() {
        check_carried_slot_join_laws("unify_param_ty", crate::analyze::unify_param_ty);
    }

    /// Two pending observations: the result must not depend on which
    /// one arrived first (it kept the later one).
    #[test]
    fn param_slot_join_of_two_pending_values_commutes() {
        let (v1, v2) = (Ty::Var { var: TyVar(1) }, Ty::Var { var: TyVar(2) });
        let join = crate::analyze::unify_param_ty;
        assert_eq!(join(v1.clone(), v2.clone()), join(v2, v1));
    }

    /// An `untyped` arm inside a union is classified like a bare
    /// `untyped`, so the answer doesn't depend on whether a producer
    /// joined `Int` and `Str` first.
    #[test]
    fn param_slot_join_classifies_untyped_arms_inside_unions() {
        let join = crate::analyze::unify_param_ty;
        let str_or_untyped = Ty::Union { variants: vec![Ty::Str, Ty::Untyped].into() };
        assert_eq!(
            join(Ty::Int, str_or_untyped),
            join(join(Ty::Int, Ty::Str), Ty::Untyped),
        );
    }

    /// `nil` alone doesn't absorb `untyped` (#617, rule (a)): a nil
    /// observation beside an untyped one is no evidence the parameter
    /// is always nil. A non-nil concrete type still absorbs it.
    #[test]
    fn param_slot_join_keeps_untyped_beside_nil_alone() {
        let join = crate::analyze::unify_param_ty;
        let nil_or_untyped = Ty::Union { variants: vec![Ty::Untyped, Ty::Nil].into() };
        assert_eq!(join(Ty::Nil, Ty::Untyped), nil_or_untyped);
        assert_eq!(join(Ty::Untyped, Ty::Nil), nil_or_untyped);
        let str_or_nil = Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() };
        assert_eq!(join(nil_or_untyped.clone(), Ty::Str), str_or_nil);
        assert_eq!(join(Ty::Str, nil_or_untyped), str_or_nil);
    }

    /// A parameter observed as `{}` at one call site and as a filled
    /// Hash at another is that Hash, as for an ivar.
    #[test]
    fn param_slot_join_drops_pending_inside_hash_spines() {
        let filled = Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(Ty::Int) };
        assert_eq!(crate::analyze::unify_param_ty(empty_hash(), filled.clone()), filled);
    }

    #[test]
    fn ivar_slot_join_is_a_lattice_join() {
        check_carried_slot_join_laws("join_ivar_slot", join_ivar_slot);
    }

    /// `@ivar = <ty>` with the value's type already stamped.
    fn ivar_write(name: &str, ty: Ty) -> Expr {
        let mut value = synth(ExprNode::Lit { value: Literal::Nil });
        value.ty = Some(ty);
        synth(ExprNode::Assign { target: LValue::Ivar { name: Symbol::from(name) }, value })
    }

    /// `@ivar[key] = <ty>`, the `[]=` Send form.
    fn ivar_index_write(name: &str, ty: Ty) -> Expr {
        let mut key = synth(ExprNode::Lit { value: Literal::Str { value: "k".to_string() } });
        key.ty = Some(Ty::Str);
        let mut value = synth(ExprNode::Lit { value: Literal::Nil });
        value.ty = Some(ty);
        synth(ExprNode::Send {
            recv: Some(synth(ExprNode::Ivar { name: Symbol::from(name) })),
            method: Symbol::from("[]="),
            args: vec![key, value],
            block: None,
            parenthesized: false,
        })
    }

    /// The ivar's harvested type after `stmts`, run in the order given.
    fn harvested_ivar(name: &str, stmts: Vec<Expr>) -> Ty {
        let mut out = HashMap::default();
        crate::analyze::extract_ivar_assignments(&synth(ExprNode::Seq { exprs: stmts }), &mut out);
        out.remove(&Symbol::from(name)).expect("ivar harvested")
    }

    fn empty_hash() -> Ty {
        Ty::Hash { key: std::sync::Arc::new(Ty::Var { var: TyVar(0) }), value: std::sync::Arc::new(Ty::Var { var: TyVar(0) }) }
    }

    /// `@h = {}` and `@h[k] = v` give one type whichever is harvested
    /// first: the `{}` literal's `Var`s are pending in both joins.
    #[test]
    fn empty_hash_seed_and_index_writes_join_in_any_order() {
        let seed_first = harvested_ivar("h", vec![
            ivar_write("h", empty_hash()),
            ivar_index_write("h", Ty::Int),
            ivar_index_write("h", Ty::Str),
        ]);
        let writes_first = harvested_ivar("h", vec![
            ivar_index_write("h", Ty::Int),
            ivar_index_write("h", Ty::Str),
            ivar_write("h", empty_hash()),
        ]);
        let want = Ty::Hash {
            key: std::sync::Arc::new(Ty::Str),
            value: std::sync::Arc::new(Ty::Union { variants: vec![Ty::Int, Ty::Str].into() }),
        };
        assert_eq!(seed_first, want);
        assert_eq!(writes_first, want);
    }

    /// A nullable Hash ivar (`@data = nil` on one path, `{}` on
    /// another) still records its `[]=` writes. Ignoring them left
    /// `Hash[Var, Var] | Nil` (or the first write's value type) after
    /// a `String` and an `Integer` were stored, the shape of the
    /// unsound `@data` typing on #617's soundness frontier.
    #[test]
    fn index_writes_widen_a_nullable_hash_ivar() {
        let want = Ty::Union {
            variants: vec![
                Ty::Hash {
                    key: std::sync::Arc::new(Ty::Str),
                    value: std::sync::Arc::new(Ty::Union { variants: vec![Ty::Int, Ty::Str].into() }),
                },
                Ty::Nil,
            ].into(),
        };
        let seeded = harvested_ivar("data", vec![
            ivar_write("data", Ty::Nil),
            ivar_write("data", empty_hash()),
            ivar_index_write("data", Ty::Str),
            ivar_index_write("data", Ty::Int),
        ]);
        assert_eq!(seeded, want);
        let written_first = harvested_ivar("data", vec![
            ivar_index_write("data", Ty::Str),
            ivar_write("data", Ty::Nil),
            ivar_index_write("data", Ty::Int),
            ivar_write("data", empty_hash()),
        ]);
        assert_eq!(written_first, want);
    }

    /// The widening still leaves an ivar holding a class instance (or
    /// a nullable one) alone: that class's own `[]=` runs, not Hash's.
    #[test]
    fn index_writes_leave_a_class_instance_ivar_alone() {
        let foo = Ty::Class { id: ClassId(Symbol::from("Foo")), args: vec![].into() };
        let nullable_foo = Ty::Union { variants: vec![foo, Ty::Nil].into() };
        let got = harvested_ivar("x", vec![
            ivar_write("x", nullable_foo.clone()),
            ivar_index_write("x", Ty::Int),
        ]);
        assert_eq!(got, nullable_foo);
    }

    /// Every order of `stmts`, for the order-independence checks below.
    fn permutations(stmts: Vec<Expr>) -> Vec<Vec<Expr>> {
        if stmts.len() <= 1 {
            return vec![stmts];
        }
        let mut out = Vec::new();
        for i in 0..stmts.len() {
            let mut rest = stmts.clone();
            let first = rest.remove(i);
            for mut tail in permutations(rest) {
                tail.insert(0, first.clone());
                out.push(tail);
            }
        }
        out
    }

    /// A `[]=` write harvested before the `{}` seed is kept, whatever
    /// the slot holds when it arrives (nothing, `nil` or a pending
    /// `Var`): the harvest gives one type in every statement order.
    #[test]
    fn index_writes_before_the_hash_seed_are_kept_in_any_order() {
        let want_nullable = Ty::Union {
            variants: vec![
                Ty::Hash {
                    key: std::sync::Arc::new(Ty::Str),
                    value: std::sync::Arc::new(Ty::Union { variants: vec![Ty::Int, Ty::Str].into() }),
                },
                Ty::Nil,
            ].into(),
        };
        let stmts = vec![
            ivar_write("data", Ty::Nil),
            ivar_write("data", empty_hash()),
            ivar_index_write("data", Ty::Str),
            ivar_index_write("data", Ty::Int),
        ];
        for order in permutations(stmts) {
            assert_eq!(harvested_ivar("data", order.clone()), want_nullable, "order {order:?}");
        }
        let want = Ty::Hash { key: std::sync::Arc::new(Ty::Str), value: std::sync::Arc::new(Ty::Int) };
        let stmts = vec![
            ivar_write("data", Ty::Var { var: TyVar(3) }),
            ivar_write("data", empty_hash()),
            ivar_index_write("data", Ty::Int),
        ];
        for order in permutations(stmts) {
            assert_eq!(harvested_ivar("data", order.clone()), want, "order {order:?}");
        }
    }

    /// A class instance's `[]=` is that class's, before or after the
    /// instance is assigned.
    #[test]
    fn index_writes_leave_a_class_instance_ivar_alone_in_any_order() {
        let foo = Ty::Class { id: ClassId(Symbol::from("Foo")), args: vec![].into() };
        let nullable_foo = Ty::Union { variants: vec![foo, Ty::Nil].into() };
        for order in permutations(vec![ivar_write("x", nullable_foo.clone()), ivar_index_write("x", Ty::Int)]) {
            assert_eq!(harvested_ivar("x", order), nullable_foo);
        }
    }

    #[test]
    fn concrete_value_shapes_are_truthy_for_boolean_operators() {
        let values = [
            Ty::Date,
            Ty::Tuple { elems: vec![Ty::Int].into() },
            Ty::Record { row: Row::default() },
            Ty::Fn {
                params: Vec::new().into(), block: None, ret: std::sync::Arc::new(Ty::Str),
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
            Ty::Tuple { elems: vec![Ty::Int].into() },
            Ty::Record { row: Row::default() },
            Ty::Fn {
                params: Vec::new().into(), block: None, ret: std::sync::Arc::new(Ty::Str),
                effects: crate::effect::EffectSet::pure(),
            },
        ] {
            let left = Ty::Union { variants: vec![truthy, Ty::Nil].into() };
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
        let h_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Str) };
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

    fn declared_sym_str_hash() -> Ty {
        Ty::Hash {
            key: std::sync::Arc::new(Ty::Sym),
            value: std::sync::Arc::new(Ty::Str),
        }
    }

    fn empty_hash_lit() -> Expr {
        synth(ExprNode::Hash {
            entries: vec![],
            kwargs: false,
        })
    }

    fn assert_stamped_sym_str_hash(e: &Expr) {
        match e.ty.as_ref() {
            Some(Ty::Hash { key, value }) => {
                assert!(matches!(key.as_ref(), Ty::Sym), "key={key:?}");
                assert!(matches!(value.as_ref(), Ty::Str), "value={value:?}");
            }
            other => panic!("expected Hash[Symbol, String], got {other:?}"),
        }
    }

    /// Declared Hash return must stamp both arms of a trailing `If`
    /// that yields bare `{}` (CodeRabbit: stop-at-If gap).
    #[test]
    fn stamp_empty_return_covers_if_branch_tails() {
        let expected = declared_sym_str_hash();
        let mut body = synth(ExprNode::If {
            cond: var("flag"),
            then_branch: empty_hash_lit(),
            else_branch: empty_hash_lit(),
        });
        stamp_expected_on_empty_return_tail(&mut body, &expected);
        let ExprNode::If {
            then_branch,
            else_branch,
            ..
        } = &*body.node
        else {
            panic!("expected If");
        };
        assert_stamped_sym_str_hash(then_branch);
        assert_stamped_sym_str_hash(else_branch);
    }

    /// An earlier `return {}` in a sequence must be stamped even when
    /// the value-producing tail is something else.
    #[test]
    fn stamp_empty_return_covers_early_return_in_seq() {
        let expected = declared_sym_str_hash();
        let mut body = synth(ExprNode::Seq {
            exprs: vec![
                synth(ExprNode::Return {
                    value: empty_hash_lit(),
                }),
                nil_lit(),
            ],
        });
        stamp_expected_on_empty_return_tail(&mut body, &expected);
        let ExprNode::Seq { exprs } = &*body.node else {
            panic!("expected Seq");
        };
        let ExprNode::Return { value } = &*exprs[0].node else {
            panic!("expected Return");
        };
        assert_stamped_sym_str_hash(value);
        assert!(
            exprs[1].ty.is_none(),
            "non-return mid/tail Nil must not be stamped as Hash"
        );
    }

    /// `return {}` inside `while` exits the enclosing method and must
    /// receive the declared Hash shape (CodeRabbit follow-up).
    #[test]
    fn stamp_empty_return_covers_return_inside_while() {
        let expected = declared_sym_str_hash();
        let mut body = synth(ExprNode::While {
            cond: var("flag"),
            body: synth(ExprNode::Return {
                value: empty_hash_lit(),
            }),
            until_form: false,
        });
        stamp_expected_on_empty_return_tail(&mut body, &expected);
        let ExprNode::While { body: loop_body, .. } = &*body.node else {
            panic!("expected While");
        };
        let ExprNode::Return { value } = &*loop_body.node else {
            panic!("expected Return in While body");
        };
        assert_stamped_sym_str_hash(value);
    }

    /// `return {}` in an attached method block (`Send.block` Lambda)
    /// exits the enclosing method; stamp it. A Lambda passed as an
    /// ordinary arg is left alone.
    #[test]
    fn stamp_empty_return_covers_return_in_send_block_not_arg_lambda() {
        let expected = declared_sym_str_hash();
        let block_lambda = synth(ExprNode::Lambda {
            params: vec![],
            rest_param: None,
            extra_params: vec![],
            block_param: None,
            body: synth(ExprNode::Return {
                value: empty_hash_lit(),
            }),
            block_style: Default::default(),
        });
        let arg_lambda = synth(ExprNode::Lambda {
            params: vec![],
            rest_param: None,
            extra_params: vec![],
            block_param: None,
            body: synth(ExprNode::Return {
                value: empty_hash_lit(),
            }),
            block_style: Default::default(),
        });
        let mut body = synth(ExprNode::Send {
            recv: None,
            method: Symbol::from("each"),
            args: vec![arg_lambda],
            block: Some(block_lambda),
            parenthesized: false,
        });
        stamp_expected_on_empty_return_tail(&mut body, &expected);
        let ExprNode::Send { args, block, .. } = &*body.node else {
            panic!("expected Send");
        };
        let ExprNode::Lambda { body: block_body, .. } = &*block.as_ref().unwrap().node else {
            panic!("expected block Lambda");
        };
        let ExprNode::Return { value } = &*block_body.node else {
            panic!("expected Return in block");
        };
        assert_stamped_sym_str_hash(value);
        let ExprNode::Lambda { body: arg_body, .. } = &*args[0].node else {
            panic!("expected arg Lambda");
        };
        let ExprNode::Return { value: arg_ret } = &*arg_body.node else {
            panic!("expected Return in arg lambda");
        };
        assert!(
            arg_ret.ty.is_none(),
            "arg-position lambda return must not inherit the method Hash return"
        );
    }

    /// `lambda { return {} }` / `define_method(...) { return {} }` are
    /// closure-local (same carve-out as `collect_return_types`); do not
    /// stamp the enclosing method's Hash return onto them.
    #[test]
    fn stamp_empty_return_skips_lambda_and_define_method_blocks() {
        let expected = declared_sym_str_hash();
        for method in ["lambda", "define_method"] {
            let block = synth(ExprNode::Lambda {
                params: vec![],
                rest_param: None,
                extra_params: vec![],
                block_param: None,
                body: synth(ExprNode::Return {
                    value: empty_hash_lit(),
                }),
                block_style: Default::default(),
            });
            let mut body = synth(ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args: vec![],
                block: Some(block),
                parenthesized: false,
            });
            stamp_expected_on_empty_return_tail(&mut body, &expected);
            let ExprNode::Send { block, .. } = &*body.node else {
                panic!("expected Send");
            };
            let ExprNode::Lambda { body: block_body, .. } = &*block.as_ref().unwrap().node else {
                panic!("expected block Lambda");
            };
            let ExprNode::Return { value } = &*block_body.node else {
                panic!("expected Return in {method} block");
            };
            assert!(
                value.ty.is_none(),
                "{method} block return must not inherit the enclosing Hash return"
            );
        }
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
                _ => Some(Ty::Union { variants: kept.into() }),
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
            Some(Ty::Union { variants: vec![Ty::Time, Ty::Nil].into() })
        }
        _ => None,
    }
}

impl BodyTyper<'_> {
    // A declared constructor can return another class. Only the default
    // Class#new operation promises an instance of the receiving class.
    fn has_declared_constructor(&self, ty: Option<&Ty>) -> bool {
        let Some(Ty::Class { id, .. }) = ty else { return false };
        let mut next = Some(id);
        let mut seen = crate::det::HashSet::default();
        while let Some(id) = next {
            if !seen.insert(id) { break; }
            let Some(class) = self.classes().get(id) else { break };
            if class.declares_constructor
                || matches!(class.class_methods.get(&Symbol::from("new")), Some(Ty::Fn { .. })) {
                return true;
            }
            next = class.parent.as_ref();
        }
        false
    }
}

fn is_time_const(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Time")
}

// The empty scalar-list form stays unmodeled; the double-array form follows the admitted Params.expect_array reader.
fn expect_hash_arg_ty(recv: Option<&Expr>, recv_ty: Option<&Ty>, method: &str, args: &[crate::expr::Expr]) -> Option<Ty> {
    use crate::expr::ExprNode;
    if method != "expect" {
        return None;
    }
    let (key, value) = match recv_ty {
        Some(Ty::Hash { key, value }) => (key.clone(), value.clone()),
        Some(Ty::Class { id, .. }) if id.0.as_str() == "ActionController::Parameters" => {
            (std::sync::Arc::new(Ty::Str), std::sync::Arc::new(Ty::Untyped))
        }
        _ => return None,
    };
    let [arg] = args else { return None };
    let ExprNode::Hash { entries, .. } = &*arg.node else { return None };
    let [(resource, permitted)] = entries.as_slice() else { return None };
    let ExprNode::Array { elements, .. } = &*permitted.node else { return None };
    if let [inner] = elements.as_slice()
        && matches!(&*resource.node, ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } })
        && let ExprNode::Array { elements: fields, .. } = &*inner.node
        && !recv.is_some_and(|expr| matches!(&*expr.node,
            ExprNode::Send { recv: Some(receiver), method, args, .. }
                if matches!(&*receiver.node, ExprNode::SelfRef)
                    && method.as_str() == "params" && args.is_empty()))
        && recv.is_some_and(|expr| matches!(&*expr.node,
                ExprNode::Send { recv: None, method, args, .. }
                    if method.as_str() == "params" && args.is_empty())
                || matches!(&*expr.node, ExprNode::Ivar { name } if name.as_str() == "params"))
        && fields.iter().all(|field| match &*field.node {
            ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } } => true,
            ExprNode::Hash { entries, .. } => entries.iter().all(|(name, filter)| {
                matches!(&*name.node, ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } })
                    && matches!(&*filter.node, ExprNode::Array { elements, .. } if elements.is_empty())
            }),
            _ => false,
        })
    {
        return Some(Ty::Array { elem: Ty::Hash {
            key: Ty::Str.into(),
            value: send::param_value_ty().into(),
        }.into() });
    }
    if elements.last().is_some_and(|field| matches!(&*field.node, ExprNode::Hash { entries, .. } if !entries.is_empty()))
        && matches!(&*resource.node, ExprNode::Lit { value: Literal::Sym { .. } })
        && recv.is_some_and(|expr| matches!(&*expr.node,
            ExprNode::Send { recv: None, method, args, block: None, .. }
                if method.as_str() == "params" && args.is_empty()))
    {
        let mut fields = indexmap::IndexMap::new();
        for (index, field) in elements.iter().enumerate() {
            match &*field.node {
                ExprNode::Lit { value: Literal::Sym { value } } => { fields.insert(value.clone(), Ty::Str); }
                ExprNode::Hash { entries, .. } if index + 1 == elements.len() => {
                    for (name, filter) in entries {
                        let name = match &*name.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => value.clone(),
                            _ => return None,
                        };
                        let ExprNode::Array { elements: filters, .. } = &*filter.node else { return None };
                        let ty = if filters.is_empty() {
                            Ty::Array { elem: Ty::Str.into() }
                        } else if let [inner] = filters.as_slice()
                            && let ExprNode::Array { elements: nested, .. } = &*inner.node
                            && !nested.is_empty()
                            && nested.iter().all(|f| matches!(&*f.node, ExprNode::Lit { value: Literal::Sym { .. } }))
                        {
                            let record = Ty::Hash { key: Ty::Str.into(), value: send::param_value_ty().into() };
                            Ty::Union { variants: vec![
                                Ty::Array { elem: record.clone().into() },
                                Ty::Hash { key: Ty::Str.into(), value: record.into() },
                                Ty::Nil,
                            ].into() }
                        } else {
                            return None;
                        };
                        fields.insert(name, ty);
                    }
                }
                _ => return None,
            }
        }
        return Some(Ty::Record { row: Row { fields: fields.into(), rest: None } });

    }
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
    // Keep the canonical strong-params chain intact for typed factory
    // discovery. Nested indexed values still use the request-value runtime.
    if method.as_str() == "permit"
        && matches!(&*recv.node, ExprNode::Send { recv: Some(root), method, .. }
            if method.as_str() == "require" && matches!(&*root.node,
                ExprNode::Send { recv: None, method, args, .. }
                if method.as_str() == "params" && args.is_empty()))
    {
        return false;
    }
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
    // The fork models the full parameter-value union before this
    // contextual runtime lowering, including nested Parameters/uploads.
    let parameter_value = recv_ty.is_some_and(|ty| match ty {
        Ty::Union { variants } => variants.iter().any(|v|
            matches!(v, Ty::Class { id, .. } if id.0.as_str() == "ActionController::Parameters")),
        _ => false,
    });
    let lowered = untyped && is_ivar_params_rooted(recv, locals);
    if !((stringish || parameter_value) && is_params_rooted(recv, locals)) && !lowered {
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

/// `[a, b].min` / `.max` (no count, no block) on an array LITERAL: the
/// literal has at least one element, so the extremum is an element, never
/// the `nil` an empty collection would give. `[x, LIMIT].min` is the
/// clamp idiom; typing it `T?` (as `Enumerable#min` does for an arbitrary
/// array) makes every `MAX - [x, LIMIT].min` look like arithmetic on nil.
///
/// Nil is stripped from the element type only when some element excludes
/// Nil: comparing nil with a non-nil value raises, so a nil result cannot
/// occur. A single nilable element (`[maybe].min`) or an all-nilable
/// literal can still return nil without a comparison error.
fn rooted_in_refused_constant(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Const { .. } => matches!(
            &expr.diagnostic(),
            Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. }) if construct.as_str() == "constant"
        ),
        ExprNode::Send { recv: Some(recv), .. } => rooted_in_refused_constant(recv),
        _ => false,
    }
}

fn literal_extremum_ty(recv: Option<&Expr>, recv_ty: &Ty, method: &Symbol, args: &[Expr]) -> Option<Ty> {
    if !matches!(method.as_str(), "min" | "max") || !args.is_empty() {
        return None;
    }
    let recv = recv?;
    let ExprNode::Array { elements, .. } = &*recv.node else { return None };
    if elements.is_empty() {
        return None;
    }
    let Ty::Array { elem } = recv_ty else { return None };
    // A nil result needs every element to be nilable. One non-nilable
    // element makes a nil result raise instead of returning nil.
    let some_non_nil = elements.iter().any(|e| match e.ty.as_ref() {
        Some(Ty::Nil) | None => false,
        Some(Ty::Union { variants }) => !variants.iter().any(|v| matches!(v, Ty::Nil)),
        Some(_) => true,
    });
    if !some_non_nil {
        return None;
    }
    Some(match &**elem {
        Ty::Union { variants } => {
            let kept: Vec<Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).cloned().collect();
            match kept.len() {
                0 => return None,
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept.into() },
            }
        }
        other => other.clone(),
    })
}

// Exact top-level stdlib names used by receiver dispatch.
pub(super) const RUBY_TOP_LEVEL: &[&str] = &[
    "Base64", "Benchmark", "BigDecimal", "CGI", "CSV", "Comparable", "Complex", "Coverage",
    "Date", "DateTime", "Digest", "Dir", "ERB", "Encoding", "Enumerable", "Errno", "Etc",
    "Fiber", "File", "FileUtils", "Find", "Forwardable", "GC", "IO", "IPAddr", "JSON",
    "Kernel", "Logger", "Marshal", "Math", "Monitor", "Mutex", "Net", "ObjectSpace", "Open3",
    "OpenSSL", "OpenStruct", "PP", "Pathname", "Prism", "Process", "Psych", "Random",
    "Rational", "Ripper", "SecureRandom", "Set", "Shellwords", "Signal", "Singleton",
    "Socket", "StringIO", "Struct", "Tempfile", "Thread", "Time", "Timeout", "URI", "YAML",
    "Zlib",
];

/// `Array[String]` — what `ActiveSupport.presence_in(value, list)` declares for its list.
fn is_string_array(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Array { elem }) if **elem == Ty::Str)
}

/// A value the program drops: the expression itself and, through an `if`'s
/// branches and a sequence's last statement, the one that would have
/// answered it.
fn mark_discarded(e: &mut Expr) {
    set_discarded(e, true);
}

fn set_discarded(e: &mut Expr, discarded: bool) {
    if discarded {
        e.decisions |= crate::expr::DISCARDED_VALUE;
    } else {
        e.decisions &= !crate::expr::DISCARDED_VALUE;
    }
    match &mut *e.node {
        ExprNode::If { then_branch, else_branch, .. } => {
            set_discarded(then_branch, discarded);
            set_discarded(else_branch, discarded);
        }
        ExprNode::Seq { exprs } => {
            if let Some(last) = exprs.last_mut() {
                set_discarded(last, discarded);
            }
        }
        _ => {}
    }
}
