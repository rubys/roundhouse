use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol, TyVar};

/// The types that inhabit Roundhouse values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Sym,
    Nil,

    Array { elem: Box<Ty> },
    Hash { key: Box<Ty>, value: Box<Ty> },
    Tuple { elems: Vec<Ty> },
    Record { row: Row },
    Union { variants: Vec<Ty> },

    Class { id: ClassId, args: Vec<Ty> },

    Fn {
        params: Vec<Param>,
        block: Option<Box<Ty>>,
        ret: Box<Ty>,
        effects: EffectSet,
    },

    Var { var: TyVar },

    /// RBS `untyped` — gradual-typing escape hatch. Distinct from
    /// `Ty::Var` (inference gap) in intent: `Untyped` is an
    /// author-signed declaration that this position opts out of
    /// checking, while `Var` means the analyzer couldn't determine a
    /// type.
    ///
    /// Propagation: dispatching a method on `Untyped` returns
    /// `Untyped`, so the gradual choice flows through the IR
    /// unconditionally.
    ///
    /// Targets that admit a gradual escape hatch (TypeScript `any`,
    /// Python no-annotation, Elixir dynamic dispatch) emit `Untyped`
    /// nodes cleanly. Strict targets (Rust, Go) are expected to
    /// elevate any reachable `Untyped` to an emit-time error via the
    /// diagnostic pipeline — the gradual escape only survives
    /// emission for targets that explicitly accept it.
    Untyped,
}

/// A row-polymorphic record shape.
/// `fields` are known; `rest` is the open-extension variable if this is a partial view.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub fields: IndexMap<Symbol, Ty>,
    pub rest: Option<TyVar>,
}

impl Row {
    pub fn closed() -> Self {
        Row { fields: IndexMap::new(), rest: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: Symbol,
    pub ty: Ty,
    pub kind: ParamKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamKind {
    Required,
    Optional,
    Rest,
    Keyword { required: bool },
    KeywordRest,
    Block,
}
