use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::effect::EffectSet;
use crate::expr::{Expr, Literal};
use crate::ident::{ClassId, Symbol, TableRef};
use crate::span::Span;
use crate::ty::{Row, Ty};

/// A source comment preserved through the pipeline. We inline comments
/// on the owning IR node (`leading_comments` / `trailing_comment` fields)
/// rather than keep a side-table keyed by node identity the way ruby2js
/// does — our IR isn't identity-stable across transforms.
///
/// `text` is the full original including the leading `#` (line form) or
/// `=begin` / `=end` markers (block form). Emitters translate to the
/// target's native comment syntax when needed; the IR stays source-native.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub text: String,
    #[serde(default, skip_serializing_if = "Span::is_synthetic")]
    pub span: Span,
}

// Models ----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub name: ClassId,
    /// `None` only for anonymous/top-level classes we haven't resolved;
    /// real Rails models always inherit from `ApplicationRecord` (or
    /// `ActiveRecord::Base` for `ApplicationRecord` itself). Needed so
    /// the Ruby emitter reproduces the source's superclass verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ClassId>,
    pub table: TableRef,
    pub attributes: Row,
    /// Source-ordered class body. The Ruby emitter re-emits entries in
    /// order, so the preserved sequence is what determines byte-for-byte
    /// round-trip. Filter via the accessors (`associations()`,
    /// `validations()`, …) when the specialized view is what you want.
    pub body: Vec<ModelBodyItem>,
    /// Span of the `class … end` declaration in the model source. The
    /// file-grain fallback for synthesized method bodies whose inputs
    /// carry no finer span (schema-derived accessors, adapter
    /// primitives, `dom_prefix`).
    #[serde(default, skip_serializing_if = "Span::is_synthetic")]
    pub span: Span,
}

impl Model {
    pub fn associations(&self) -> impl Iterator<Item = &Association> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Association { assoc, .. } => Some(assoc),
            _ => None,
        })
    }

    /// Like `associations()`, but paired with each declaration's source
    /// span — for lowerers that stamp synthesized methods with the
    /// `has_many`/`belongs_to` line they came from.
    pub fn spanned_associations(&self) -> impl Iterator<Item = (Span, &Association)> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Association { assoc, .. } => Some((item.span(), assoc)),
            _ => None,
        })
    }

    pub fn validations(&self) -> impl Iterator<Item = &Validation> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Validation { validation, .. } => Some(validation),
            _ => None,
        })
    }

    /// `validations()` paired with each `validates` call's source span.
    pub fn spanned_validations(&self) -> impl Iterator<Item = (Span, &Validation)> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Validation { validation, .. } => Some((item.span(), validation)),
            _ => None,
        })
    }

    pub fn scopes(&self) -> impl Iterator<Item = &Scope> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Scope { scope, .. } => Some(scope),
            _ => None,
        })
    }

    pub fn scopes_mut(&mut self) -> impl Iterator<Item = &mut Scope> {
        self.body.iter_mut().filter_map(|item| match item {
            ModelBodyItem::Scope { scope, .. } => Some(scope),
            _ => None,
        })
    }

    pub fn callbacks(&self) -> impl Iterator<Item = &Callback> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Callback { callback, .. } => Some(callback),
            _ => None,
        })
    }

    pub fn methods(&self) -> impl Iterator<Item = &MethodDef> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Method { method, .. } => Some(method),
            _ => None,
        })
    }

    pub fn methods_mut(&mut self) -> impl Iterator<Item = &mut MethodDef> {
        self.body.iter_mut().filter_map(|item| match item {
            ModelBodyItem::Method { method, .. } => Some(method),
            _ => None,
        })
    }
}

/// One statement inside a model's class body, in source order. Known DSL
/// calls (associations, validations, …) become their typed variants;
/// anything else falls through to `Unknown` so it can be re-emitted
/// verbatim. The `Unknown` fallback is the *whole reason* this type
/// exists — without it the Ruby emitter silently drops every
/// unrecognized line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum ModelBodyItem {
    Association {
        assoc: Association,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
        /// Span of the recognized DSL call (`has_many :comments, …`).
        /// The typed variants drop the source `Expr`, so the span rides
        /// the wrapper — synthesized methods inherit it (see
        /// `lower::model_to_library`).
        #[serde(default, skip_serializing_if = "Span::is_synthetic")]
        span: Span,
    },
    Validation {
        validation: Validation,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
        /// Span of the `validates …` call this rule was recognized from.
        #[serde(default, skip_serializing_if = "Span::is_synthetic")]
        span: Span,
    },
    Scope {
        scope: Scope,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
    },
    Callback {
        callback: Callback,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
        /// Span of the `before_save :…` call this hook was recognized from.
        #[serde(default, skip_serializing_if = "Span::is_synthetic")]
        span: Span,
    },
    Method {
        method: MethodDef,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
    },
    /// Class-body statement whose semantics aren't yet recognized
    /// (`broadcasts_to …`, `primary_abstract_class`, bare method calls,
    /// …). Held as a raw expression for source-faithful re-emission.
    Unknown {
        expr: Expr,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
    },
}

impl ModelBodyItem {
    /// Source span of this body item. The typed variants (Association /
    /// Validation / Callback) store the recognized call's span on the
    /// wrapper; the Expr-carrying variants read it off their payload.
    pub fn span(&self) -> Span {
        match self {
            Self::Association { span, .. }
            | Self::Validation { span, .. }
            | Self::Callback { span, .. } => *span,
            Self::Scope { scope, .. } => scope.body.span,
            Self::Method { method, .. } => method.body.span,
            Self::Unknown { expr, .. } => expr.span,
        }
    }

    /// Return the leading comments attached to this item, regardless of
    /// variant — so emit code can fetch them without re-matching.
    pub fn leading_comments(&self) -> &[Comment] {
        match self {
            Self::Association { leading_comments, .. }
            | Self::Validation { leading_comments, .. }
            | Self::Scope { leading_comments, .. }
            | Self::Callback { leading_comments, .. }
            | Self::Method { leading_comments, .. }
            | Self::Unknown { leading_comments, .. } => leading_comments,
        }
    }

    pub fn leading_comments_mut(&mut self) -> &mut Vec<Comment> {
        match self {
            Self::Association { leading_comments, .. }
            | Self::Validation { leading_comments, .. }
            | Self::Scope { leading_comments, .. }
            | Self::Callback { leading_comments, .. }
            | Self::Method { leading_comments, .. }
            | Self::Unknown { leading_comments, .. } => leading_comments,
        }
    }

    pub fn leading_blank_line(&self) -> bool {
        match self {
            Self::Association { leading_blank_line, .. }
            | Self::Validation { leading_blank_line, .. }
            | Self::Scope { leading_blank_line, .. }
            | Self::Callback { leading_blank_line, .. }
            | Self::Method { leading_blank_line, .. }
            | Self::Unknown { leading_blank_line, .. } => *leading_blank_line,
        }
    }

    pub fn set_leading_blank_line(&mut self, v: bool) {
        match self {
            Self::Association { leading_blank_line, .. }
            | Self::Validation { leading_blank_line, .. }
            | Self::Scope { leading_blank_line, .. }
            | Self::Callback { leading_blank_line, .. }
            | Self::Method { leading_blank_line, .. }
            | Self::Unknown { leading_blank_line, .. } => *leading_blank_line = v,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Association {
    BelongsTo {
        name: Symbol,
        target: ClassId,
        foreign_key: Symbol,
        optional: bool,
    },
    HasMany {
        name: Symbol,
        target: ClassId,
        foreign_key: Symbol,
        through: Option<Symbol>,
        dependent: Dependent,
    },
    HasOne {
        name: Symbol,
        target: ClassId,
        foreign_key: Symbol,
        dependent: Dependent,
    },
    HasAndBelongsToMany {
        name: Symbol,
        target: ClassId,
        join_table: Symbol,
    },
}

impl Association {
    pub fn name(&self) -> &Symbol {
        match self {
            Association::BelongsTo { name, .. }
            | Association::HasMany { name, .. }
            | Association::HasOne { name, .. }
            | Association::HasAndBelongsToMany { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dependent {
    #[default]
    None,
    Destroy,
    DestroyAsync,
    Delete,
    DeleteAll,
    Nullify,
    Restrict,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Validation {
    pub attribute: Symbol,
    pub rules: Vec<ValidationRule>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValidationRule {
    Presence,
    Absence,
    Uniqueness { scope: Vec<Symbol>, case_sensitive: bool },
    Length { min: Option<u32>, max: Option<u32> },
    Format { pattern: String },
    Numericality { only_integer: bool, gt: Option<f64>, lt: Option<f64> },
    Inclusion { values: Vec<Literal> },
    Custom { method: Symbol },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scope {
    pub name: Symbol,
    /// Lambda parameters, in source order (`scope :hottest, ->(user = nil,
    /// tags = nil) { … }`). Defaults are carried so the lowered class
    /// method reproduces them; a trailing relation parameter is appended
    /// at lowering time (see `push_scope_methods`).
    pub params: Vec<Param>,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Callback {
    pub hook: CallbackHook,
    pub target: Symbol,
    pub condition: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallbackHook {
    BeforeValidation,
    AfterValidation,
    BeforeSave,
    AfterSave,
    BeforeCreate,
    AfterCreate,
    BeforeUpdate,
    AfterUpdate,
    BeforeDestroy,
    AfterDestroy,
    AfterCommit,
    AfterRollback,
}

/// A formal parameter on a `MethodDef`. Carries the parameter name and an
/// optional default expression. Today only positional-with-default is
/// represented; keyword variants are tracked as a future gap (see
/// `project_lowered_ir_gaps_for_runnability`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: Symbol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Expr>,
}

impl Param {
    pub fn positional(name: Symbol) -> Self {
        Self { name, default: None }
    }

    pub fn with_default(name: Symbol, default: Expr) -> Self {
        Self { name, default: Some(default) }
    }

    pub fn as_str(&self) -> &str {
        self.name.as_str()
    }
}

impl std::fmt::Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MethodDef {
    pub name: Symbol,
    pub receiver: MethodReceiver,
    pub params: Vec<Param>,
    /// Block parameter declared at the `def` site (`def foo(x, &block)`).
    /// Distinct from `params` because it occupies the call-site `block:`
    /// slot, never `args:`. Present only when the method binds an
    /// incoming block to a name; methods that `yield` without naming the
    /// block, or that take no block at all, carry `None`. Default exists
    /// in IR today but is not yet consumed by analyzer/emit — landed
    /// ahead of `ExprNode::ProcRef` work (issue #25) so construction
    /// sites can be swept in one commit, decoupled from the
    /// Proc-as-value semantics that will read this field later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_param: Option<Param>,
    pub body: Expr,
    pub signature: Option<Ty>,
    pub effects: EffectSet,
    /// Class/module the method is defined under, if any. `None` for
    /// top-level `def`s. Used by the body-typer to seed `self_ty` when
    /// analyzing library-shape code (runtime_src) — Rails app ingest
    /// holds this info on the enclosing Model/Controller struct
    /// instead. Carried as the last-segment name (e.g. `Base`, not
    /// `ActiveRecord::Base`), matching how `Const { path }` types.
    #[serde(default)]
    pub enclosing_class: Option<Symbol>,
    /// Calling-convention intent — Ruby blurs attribute access and
    /// zero-arg method calls (`obj.foo` could be either), but TS,
    /// Rust, Go, and Crystal need the distinction at emit time.
    /// Lowerers and ingest record what they know by construction;
    /// emitters consume to decide getter/field syntax vs method-call
    /// parens. Defaults to `Method` for backward compatibility with
    /// older serializations and for source-defined methods that
    /// don't tag themselves.
    #[serde(default)]
    pub kind: AccessorKind,
    /// True when this method must be awaited (TS `async`, Rust
    /// `async fn`, Python `async def`). Set by Phase 1's seed
    /// pass for methods named in the active deployment profile's
    /// adapter manifest, and grown by Phase 2's fixed-point
    /// propagation through the call graph. Always `false` under
    /// the default `node-sync` profile — the seed list is empty,
    /// nothing propagates, emit is unchanged.
    #[serde(default)]
    pub is_async: bool,
    /// True when the method body mutates instance state — either
    /// directly (`@ivar = …`, `self[k] = v`, `self.attr = v`) or
    /// transitively (calls another method on `self` that does).
    /// Filled by `analyze::mutates_self` as an IR-side annotation;
    /// strict-typed targets (Rust `&mut self`, Crystal `def` vs
    /// mutating-flag conventions, future Kotlin/Swift) read the
    /// flag to pick the receiver shape at emit time. Permissive
    /// targets (TS, Ruby) ignore it. Defaults to `false`; the
    /// analyze pass overrides per class via the transitive walk.
    #[serde(default)]
    pub mutates_self: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessorKind {
    /// Real method — call with parens (`obj.foo()` in TS, `obj.foo()`
    /// in Rust, etc.). The default. Source-defined `def foo` lands here
    /// unless it's recognized as an attr_reader/writer pattern.
    #[default]
    Method,
    /// Reads as a field/property/getter. Zero-arg, no side effects,
    /// body conceptually pure data access (an `@ivar`, a frozen
    /// constant, a derived value). TS: `get foo()` or bare `foo: T`
    /// field; Rust: a field read; Crystal: getter macro form.
    /// Synthesized by `attr_reader`/`attr_accessor` lowering and by
    /// has_many/belongs_to association readers.
    AttributeReader,
    /// Writes a field. Single param, body assigns to a field.
    /// TS: `set foo(v)` or field assign; Rust: field write; Crystal:
    /// setter macro. Synthesized by `attr_writer`/`attr_accessor`
    /// lowering.
    AttributeWriter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodReceiver {
    Instance,
    Class,
}

// Library classes -------------------------------------------------------

/// A non-model class living under `app/models/`. Surfaced by lowerings
/// like has_many specialization (`ArticleCommentsProxy`) — the file is
/// in the models directory but the class doesn't extend
/// `ApplicationRecord` / `ActiveRecord::Base`, so the model emission
/// path's table-name/columns/modelRegistry scaffolding doesn't apply.
/// Emitted as a plain class in each target language.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryClass {
    pub name: ClassId,
    /// True when the source declared this with `module` rather than
    /// `class`. Carried because Ruby (and Spinel) require the
    /// distinction at use sites: `include X` works only on a Module
    /// and raises TypeError on a Class. Without this flag, mixin
    /// modules emitted as classes would fail to compile when their
    /// including class hits the `include` call.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_module: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ClassId>,
    /// `include` directives at the class top level, in source order
    /// (e.g. `Enumerable`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<ClassId>,
    /// All instance + class methods in the class. `attr_reader` /
    /// `attr_writer` / `attr_accessor` are lowered to synthetic
    /// `MethodDef`s at ingest time (not preserved as attr declarations
    /// — surface form is sacrificed for downstream uniformity per the
    /// lowerer-first architecture).
    pub methods: Vec<MethodDef>,
    /// Provenance tag for synthesized classes. `None` for source-derived
    /// classes; populated when the lowerer creates per-resource
    /// specializations (e.g. `<Model>Row`, `<Resource>Params`) so future
    /// per-target collapsers can group structurally-identical instances
    /// without rerunning equivalence detection. The tag carries the
    /// originating template plus the (resource, fields) tuple that
    /// instantiated it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<LibraryClassOrigin>,
    /// Class-level constant definitions (`NAME = <expr>`), in source
    /// order. Carried from a controller's / model's class body so refs
    /// like `ApplicationController::TAG_FILTER_COOKIE` or
    /// `Story::COMMENTABLE_DAYS` resolve. Emitted before the methods.
    /// Most synthesized classes have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constants: Vec<(Symbol, Expr)>,
}

/// What synthesized a `LibraryClass`. Used by per-target collapsers to
/// fold structurally-equivalent instances back to a generic shape (e.g.
/// `Record<string, FieldType>`-style narrowing in TS) when the target
/// can express it. Per `project_specialization_strategy.md`, collapse
/// is per-emitter, not per-lowerer; the IR carries enough info for any
/// emitter to make the call without re-detecting equivalence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "template", rename_all = "snake_case")]
pub enum LibraryClassOrigin {
    /// Per-resource params holder synthesized from a controller's
    /// `permit([:f1, :f2, …])` declaration. `resource` is the singular
    /// model name (e.g. `:article`); `fields` is the permitted column
    /// list in declaration order.
    ResourceParams {
        resource: Symbol,
        fields: Vec<Symbol>,
    },
    /// Per-model row holder synthesized from a model's schema columns.
    /// `resource` is the singular model name; `fields` is every column
    /// in schema order (id, …, created_at, updated_at).
    ResourceRow {
        resource: Symbol,
        fields: Vec<Symbol>,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A top-level callable: no instance state, no inheritance, fully
/// resolvable at the call site as `<module_path>.<name>(args)`.
/// Per-target emitters pick the idiomatic surface form:
///
/// - Spinel / Crystal / Ruby: class method on a module
///   (`module Views::Articles; def self.article(a); …; end; end`)
/// - TypeScript / Python: exported function in a module file
///   (`export function article(a: Article): string { … }`)
/// - Rust / Go: package-level function (`pub fn article(a: &Article) -> String`)
/// - Elixir: `def` inside a `defmodule` (the file = module)
///
/// The IR commits to the semantics; the surface form is the
/// emitter's call. Per-template view bodies are the canonical
/// producer; `RouteHelpers` / `Importmap` / `Schema` / `Seeds` are
/// future migrations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryFunction {
    /// Module path the function lives under, e.g.
    /// `["Views", "Articles"]`. Empty for top-level (rare; views
    /// always have at least one segment).
    pub module_path: Vec<crate::ident::Symbol>,
    /// The function's own name, e.g. `"article"`. Together with
    /// `module_path` forms the dispatch key
    /// (`Views::Articles.article`).
    pub name: crate::ident::Symbol,
    pub params: Vec<Param>,
    pub body: Expr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<crate::ty::Ty>,
    #[serde(default, skip_serializing_if = "is_pure_effects")]
    pub effects: crate::effect::EffectSet,
    /// Set by Phase 2's async-color propagation when this function's
    /// body calls (transitively) a method in the active deployment
    /// profile's adapter manifest. Drives `export async function`
    /// emission and `Promise<T>` return-type wrapping. Defaults to
    /// false, so non-async profiles emit byte-equivalent to pre-
    /// Phase-3.
    #[serde(default)]
    pub is_async: bool,
}

fn is_pure_effects(e: &crate::effect::EffectSet) -> bool {
    e.effects.is_empty()
}

// Controllers -----------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Controller {
    pub name: ClassId,
    pub parent: Option<ClassId>,
    /// Source-ordered class body. Same shape as `Model.body` — the
    /// emitter iterates in order so `private` markers land at the right
    /// position and unknown class-body calls round-trip verbatim.
    pub body: Vec<ControllerBodyItem>,
    /// Layout declaration from `layout :foo` / `layout "foo"` /
    /// `layout false`. Absent → `Inherit` (walk parent chain; final
    /// fallback is `layouts/application` per Rails convention).
    /// Used by analyze to seed ivar types into layout views from the
    /// union of actions that render through this controller.
    #[serde(default, skip_serializing_if = "LayoutDecl::is_inherit")]
    pub layout: LayoutDecl,
    /// Empty-bodied top-level classes declared alongside the controller
    /// in its source file, as (name, parent) pairs — lobsters'
    /// `login_controller.rb` opens with `class LoginFailedError <
    /// StandardError; end` and four siblings that the actions
    /// raise/rescue. Only the empty-body shape is captured (a pure
    /// declaration); a sibling with real methods stays dropped and
    /// surfaces through diagnostics as before. The Ruby emit path
    /// re-declares these ahead of the controller class.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sibling_classes: Vec<(Symbol, Symbol)>,
}

/// What `layout` was declared at the controller class level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutDecl {
    /// No `layout` declaration. Effective layout comes from the parent
    /// chain or convention default (`layouts/application`).
    #[default]
    Inherit,
    /// `layout :foo` or `layout "foo"`. Resolves to `layouts/<name>`.
    Name { name: Symbol },
    /// `layout false` / `layout nil` — render bare, no layout.
    None,
}

impl LayoutDecl {
    pub fn is_inherit(&self) -> bool {
        matches!(self, LayoutDecl::Inherit)
    }
}

impl Controller {
    pub fn filters(&self) -> impl Iterator<Item = &Filter> {
        self.body.iter().filter_map(|item| match item {
            ControllerBodyItem::Filter { filter, .. } => Some(filter),
            _ => None,
        })
    }

    pub fn actions(&self) -> impl Iterator<Item = &Action> {
        self.body.iter().filter_map(|item| match item {
            ControllerBodyItem::Action { action, .. } => Some(action),
            _ => None,
        })
    }

    pub fn actions_mut(&mut self) -> impl Iterator<Item = &mut Action> {
        self.body.iter_mut().filter_map(|item| match item {
            ControllerBodyItem::Action { action, .. } => Some(action),
            _ => None,
        })
    }
}

/// One statement inside a controller class body, in source order.
/// Same rationale as `ModelBodyItem`: known forms get typed variants,
/// everything else falls through to `Unknown` for faithful re-emission.
/// `PrivateMarker` is a zero-payload marker for the bare `private`
/// keyword — methods following it in source are private by Ruby's
/// visibility rules; the marker carries the position, not the
/// visibility of individual actions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum ControllerBodyItem {
    Filter {
        filter: Filter,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
    },
    Action {
        action: Action,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
    },
    PrivateMarker {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
    },
    Unknown {
        expr: Expr,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
    },
}

impl ControllerBodyItem {
    pub fn leading_comments(&self) -> &[Comment] {
        match self {
            Self::Filter { leading_comments, .. }
            | Self::Action { leading_comments, .. }
            | Self::PrivateMarker { leading_comments, .. }
            | Self::Unknown { leading_comments, .. } => leading_comments,
        }
    }

    pub fn leading_comments_mut(&mut self) -> &mut Vec<Comment> {
        match self {
            Self::Filter { leading_comments, .. }
            | Self::Action { leading_comments, .. }
            | Self::PrivateMarker { leading_comments, .. }
            | Self::Unknown { leading_comments, .. } => leading_comments,
        }
    }

    pub fn leading_blank_line(&self) -> bool {
        match self {
            Self::Filter { leading_blank_line, .. }
            | Self::Action { leading_blank_line, .. }
            | Self::PrivateMarker { leading_blank_line, .. }
            | Self::Unknown { leading_blank_line, .. } => *leading_blank_line,
        }
    }

    pub fn set_leading_blank_line(&mut self, v: bool) {
        match self {
            Self::Filter { leading_blank_line, .. }
            | Self::Action { leading_blank_line, .. }
            | Self::PrivateMarker { leading_blank_line, .. }
            | Self::Unknown { leading_blank_line, .. } => *leading_blank_line = v,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub kind: FilterKind,
    pub target: Symbol,
    pub only: Vec<Symbol>,
    pub except: Vec<Symbol>,
    /// Surface style of `only: [...]` — brackets (`[:a, :b]`) vs
    /// `%i[a b]`. Only meaningful when `only` is non-empty.
    #[serde(default)]
    pub only_style: crate::expr::ArrayStyle,
    /// Surface style of `except: [...]`. Only meaningful when
    /// `except` is non-empty.
    #[serde(default)]
    pub except_style: crate::expr::ArrayStyle,
    /// Symbol-form `if:` guard (`before_action :set_account, if:
    /// :account_required?` → `account_required?`). The condition is a
    /// runtime predicate the static chain can't evaluate, so analyze
    /// seeds the filter's ivars regardless — it's carried for
    /// consumers that present the chain (traceroute hops show the
    /// guard verbatim). Lambda/proc conditions are not captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_cond: Option<Symbol>,
    /// Symbol-form `unless:` guard. Same carriage rules as `if_cond`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unless_cond: Option<Symbol>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    Before,
    Around,
    After,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub name: Symbol,
    pub params: Row,
    /// Optional positional params with their default-value exprs, in
    /// declaration order (after the required `params`). Preserved so a
    /// helper method's emitted signature matches the source — e.g.
    /// `def get_from_cache(opts = {})` — rather than dropping the params
    /// and crashing the body that still reads them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opt_params: Vec<(Symbol, Expr)>,
    /// The captured block parameter name (`def f(&block)`), if the method
    /// names its block. Occupies the `def`-site `&`-slot, distinct from
    /// the positional `params`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_param: Option<Symbol>,
    pub body: Expr,
    pub renders: RenderTarget,
    pub effects: EffectSet,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderTarget {
    Template { name: Symbol, formats: Vec<Symbol> },
    Redirect { to: Expr },
    Json { value: Expr },
    Head { status: u16 },
    Inferred,
}

// Routes ----------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RouteTable {
    pub entries: Vec<RouteSpec>,
}

/// How a custom route nested inside a `resources` block attaches to the
/// parent resource — the Rails member/collection/child distinction, which
/// decides the id segment the flattener prepends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScope {
    /// A bare verb declared directly in the `resources` block (Rails
    /// nests it under the parent's `/:<singular>_id`, e.g.
    /// `post "upvote"` in `resources :stories` → `/stories/:story_id/upvote`),
    /// or any non-member/collection route. The conservative default.
    #[default]
    Nested,
    /// Declared inside `member do … end` — acts on one record, nested
    /// under the resource's own `/:id` (`/comments/:id/reply`).
    Member,
    /// Declared inside `collection do … end` — acts on the whole
    /// collection, no id segment (`/photos/search`).
    Collection,
}

impl ResourceScope {
    pub fn is_nested(&self) -> bool {
        matches!(self, ResourceScope::Nested)
    }
}

/// Surface forms of a routes.rb entry. Preserves source structure so
/// `resources :articles do ... end` round-trips byte-for-byte; a downstream
/// target emitter that needs concrete routes expands via [`RouteSpec::expand`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteSpec {
    /// Direct verb call: `get "/path", to: "controller#action"[, as: :name]`.
    /// The explicit form is the only one that can express arbitrary paths
    /// and custom constraints.
    Explicit {
        method: HttpMethod,
        path: String,
        controller: ClassId,
        action: Symbol,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        as_name: Option<Symbol>,
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        constraints: IndexMap<Symbol, String>,
        /// How this route nests under an enclosing `resources` block.
        /// Set when the route is declared inside a `member do`/
        /// `collection do` wrapper; `Nested` (the default) covers a bare
        /// verb declared directly in the block and any other case.
        #[serde(default, skip_serializing_if = "ResourceScope::is_nested")]
        scope: ResourceScope,
    },
    /// `root "controller#action"` — shorthand for `GET /` routed to the
    /// given target, with `:root` as the generated name.
    Root { target: String },
    /// `resources :name [, only: [...]] [, except: [...]] [do ... end]`.
    /// `only` and `except` are empty-on-default (an empty `only` means
    /// "all seven standard actions," matching Rails' behavior). Nested
    /// blocks hold any entries declared inside the `do ... end`, typically
    /// further `resources` calls. `singular` marks `resource :name`:
    /// no `index`, no `:id` path segment, but the controller is still
    /// the *plural* class (Rails' `resource :profile` →
    /// `ProfilesController`).
    Resources {
        name: Symbol,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        only: Vec<Symbol>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        except: Vec<Symbol>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        nested: Vec<RouteSpec>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        singular: bool,
    },
    /// `namespace :admin do … end` / `scope … do … end` — a routing
    /// scope wrapping nested entries. `namespace :x` is `scope` with
    /// all three facets set to `x`; `scope module: :web` sets only
    /// `module`. Composition happens in the flattener: `path`
    /// prepends a URL segment, `module` prefixes the controller class
    /// namespace, `as_prefix` prefixes route-helper names.
    Scope {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        module: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        as_prefix: Option<String>,
        entries: Vec<RouteSpec>,
    },
}

/// One `get "/path", to: "c#a"` entry. Kept as a standalone struct so
/// call sites that want the flat record (tests, downstream emitters) can
/// still destructure one without going through the `RouteSpec` variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub method: HttpMethod,
    pub path: String,
    pub controller: ClassId,
    pub action: Symbol,
    pub as_name: Option<Symbol>,
    pub constraints: IndexMap<Symbol, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Any,
}

// Views -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub name: Symbol,
    pub format: Symbol,
    pub locals: Row,
    pub body: Expr,
}

// Tests -----------------------------------------------------------------

/// A Ruby test file (typically `test/models/*_test.rb`). One class per
/// file, containing a sequence of `test "description" do ... end`
/// declarations. `target` is the class under test, inferred from the
/// test class's name by stripping a `Test` suffix — e.g.
/// `ArticleTest` → `Article`. `None` when the stripped name doesn't
/// match any model in the app, in which case the tests are still
/// ingested but typed emission will need the user to point at the
/// right target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestModule {
    pub name: ClassId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ClassId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ClassId>,
    pub tests: Vec<Test>,
    /// Body of `setup do ... end` or `def setup; ...; end`, if
    /// present. The lowerer inlines this at the start of each test
    /// method so the body-typer's Seq walk picks up ivar
    /// assignments (`@article = articles(:one)`) before the test's
    /// body runs. Mirror of the controller filter-inlining pattern.
    /// `None` when the test class has no setup hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<Expr>,
    /// Classes declared inside the test class body — e.g. the
    /// framework-test pattern of `class Validatable; include
    /// ActiveRecord::Validations; end` inside `class ValidationsTest
    /// < Minitest::Test`. They're scoped to the test file in Ruby;
    /// the TS emit hoists them to file scope above the test class.
    /// Empty for the typical Rails app-test (which just calls into
    /// app/models/ and doesn't redefine classes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inner_classes: Vec<LibraryClass>,
    /// Non-test, non-setup instance methods on the test class —
    /// helper methods like `setup_adapter_with_stub_row(id)` that
    /// the test methods invoke. Captured here so the lowerer can
    /// emit them as ordinary instance methods on the lowered test
    /// class. Empty when the file has no helpers (the typical case).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub helpers: Vec<MethodDef>,
    /// Class-body constant assignments — `TABLE = [...]`, `SCHEMA =
    /// {...}` declared at test-class scope. Lifted to file-scope
    /// `const NAME = <value>` declarations during emit so test methods
    /// can reference them as bare names (mirrors Ruby's lexical
    /// constant lookup). Empty for tests that don't declare constants
    /// inline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constants: Vec<(Symbol, Expr)>,
    /// Module includes at test-class scope — `include ActionDispatch`,
    /// `include ActionView`, `include ActionView::ViewHelpers`. The
    /// Ruby spinel emit replays them verbatim so bare-name refs
    /// (`Router`, `FormBuilder`) resolve under CRuby. The TS emit
    /// resolves the same refs via its framework-namespace
    /// import-stripper, so the field is informational only there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<ClassId>,
}

/// A single `test "name" do ... end` block. `name` is the literal
/// string passed to the `test` macro; `body` is the block body.
/// Emission snake-cases `name` for the target's function-name form
/// (`creates an article` → `creates_an_article`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Test {
    pub name: String,
    pub body: Expr,
}

/// A `test/fixtures/<name>.yml` file. `name` is the file stem
/// (`articles`, `comments`) — conventionally matches the table name.
/// `records` preserves the label→fields mapping order from the source;
/// values stay as strings since Rails YAML fixtures rarely type-tag
/// them and emitters can interpret per column type. Fixture-to-fixture
/// references (Rails's `article: one` shorthand for "id of the `one`
/// fixture in articles") are preserved verbatim as strings — the
/// resolver is an emit-time concern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    pub name: Symbol,
    pub records: IndexMap<Symbol, IndexMap<Symbol, String>>,
}
