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
    pub table: TableRef,
    pub attributes: Row,
    pub associations: Vec<Association>,
    pub validations: Vec<Validation>,
    pub scopes: Vec<Scope>,
    pub callbacks: Vec<Callback>,
    pub methods: Vec<MethodDef>,
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
    pub filters: Vec<Filter>,
    pub actions: Vec<Action>,
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
