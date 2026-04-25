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
}

impl Model {
    pub fn associations(&self) -> impl Iterator<Item = &Association> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Association { assoc, .. } => Some(assoc),
            _ => None,
        })
    }

    pub fn validations(&self) -> impl Iterator<Item = &Validation> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Validation { validation, .. } => Some(validation),
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
    },
    Validation {
        validation: Validation,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leading_comments: Vec<Comment>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        leading_blank_line: bool,
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
    pub params: Vec<Symbol>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MethodDef {
    pub name: Symbol,
    pub receiver: MethodReceiver,
    pub params: Vec<Symbol>,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ClassId>,
    /// `include` directives at the class top level, in source order
    /// (e.g. `Enumerable`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<ClassId>,
    pub methods: Vec<MethodDef>,
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
    },
    /// `root "controller#action"` — shorthand for `GET /` routed to the
    /// given target, with `:root` as the generated name.
    Root { target: String },
    /// `resources :name [, only: [...]] [, except: [...]] [do ... end]`.
    /// `only` and `except` are empty-on-default (an empty `only` means
    /// "all seven standard actions," matching Rails' behavior). Nested
    /// blocks hold any entries declared inside the `do ... end`, typically
    /// further `resources` calls.
    Resources {
        name: Symbol,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        only: Vec<Symbol>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        except: Vec<Symbol>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        nested: Vec<RouteSpec>,
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
