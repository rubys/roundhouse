use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::effect::EffectSet;
use crate::expr::{Expr, Literal};
use crate::ident::{ClassId, Symbol, TableRef};
use crate::ty::{Row, Ty};

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
            ModelBodyItem::Association { assoc } => Some(assoc),
            _ => None,
        })
    }

    pub fn validations(&self) -> impl Iterator<Item = &Validation> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Validation { validation } => Some(validation),
            _ => None,
        })
    }

    pub fn scopes(&self) -> impl Iterator<Item = &Scope> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Scope { scope } => Some(scope),
            _ => None,
        })
    }

    pub fn scopes_mut(&mut self) -> impl Iterator<Item = &mut Scope> {
        self.body.iter_mut().filter_map(|item| match item {
            ModelBodyItem::Scope { scope } => Some(scope),
            _ => None,
        })
    }

    pub fn callbacks(&self) -> impl Iterator<Item = &Callback> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Callback { callback } => Some(callback),
            _ => None,
        })
    }

    pub fn methods(&self) -> impl Iterator<Item = &MethodDef> {
        self.body.iter().filter_map(|item| match item {
            ModelBodyItem::Method { method } => Some(method),
            _ => None,
        })
    }

    pub fn methods_mut(&mut self) -> impl Iterator<Item = &mut MethodDef> {
        self.body.iter_mut().filter_map(|item| match item {
            ModelBodyItem::Method { method } => Some(method),
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
    Association { assoc: Association },
    Validation { validation: Validation },
    Scope { scope: Scope },
    Callback { callback: Callback },
    Method { method: MethodDef },
    /// Class-body statement whose semantics aren't yet recognized
    /// (`broadcasts_to …`, `primary_abstract_class`, bare method calls,
    /// …). Held as a raw expression for source-faithful re-emission.
    Unknown { expr: Expr },
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodReceiver {
    Instance,
    Class,
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
            ControllerBodyItem::Filter { filter } => Some(filter),
            _ => None,
        })
    }

    pub fn actions(&self) -> impl Iterator<Item = &Action> {
        self.body.iter().filter_map(|item| match item {
            ControllerBodyItem::Action { action } => Some(action),
            _ => None,
        })
    }

    pub fn actions_mut(&mut self) -> impl Iterator<Item = &mut Action> {
        self.body.iter_mut().filter_map(|item| match item {
            ControllerBodyItem::Action { action } => Some(action),
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
    Filter { filter: Filter },
    Action { action: Action },
    PrivateMarker,
    Unknown { expr: Expr },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub kind: FilterKind,
    pub target: Symbol,
    pub only: Vec<Symbol>,
    pub except: Vec<Symbol>,
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
