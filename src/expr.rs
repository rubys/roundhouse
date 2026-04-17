use serde::{Deserialize, Serialize};

use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// The core typed λ-calculus. Ruby's ~80 AST node kinds collapse into ~15 here;
/// everything else lives in the Rails dialect or is handled by normalization.
///
/// `ty` is populated by the analyzer; ingest leaves it `None`. Inline for
/// simplicity; migrate to a salsa-indexed side table when incrementality
/// becomes load-bearing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Expr {
    pub span: Span,
    pub node: Box<ExprNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<Ty>,
}

impl Expr {
    pub fn new(span: Span, node: ExprNode) -> Self {
        Self { span, node: Box::new(node), ty: None }
    }
}

fn default_true() -> bool { true }

/// Surface form of an array literal. Source fidelity: `[:a, :b]` (Brackets),
/// `%i[a b]` (PercentI, symbol list), `%w[a b]` (PercentW, word list) all
/// produce the same Prism `ArrayNode` but differ byte-for-byte in source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ArrayStyle {
    /// `[elem, elem, ...]` — the common form.
    #[default]
    Brackets,
    /// `%i[sym sym ...]` — symbol-list literal. Elements must be bare symbols.
    PercentI,
    /// `%w[word word ...]` — word-list literal. Elements must be bare strings.
    PercentW,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExprNode {
    Lit { value: Literal },
    Var { id: VarId, name: Symbol },
    /// Instance variable read: `@post`. Writes use `LValue::Ivar`.
    Ivar { name: Symbol },
    Const { path: Vec<Symbol> },
    /// Hash literal: `{ k1 => v1, k2 => v2 }` or trailing kwargs `k: v`.
    /// Keys and values are both expressions. `braced` preserves whether the
    /// source used explicit `{}` (HashNode) or the trailing-kwargs form
    /// (KeywordHashNode) — the latter only appears as the last argument of
    /// a method call.
    Hash {
        entries: Vec<(Expr, Expr)>,
        #[serde(default = "default_true")]
        braced: bool,
    },
    /// Array literal: `[a, b, c]`, `%i[a b c]`, `%w[a b c]`.
    /// `style` preserves which surface form the source used.
    Array {
        elements: Vec<Expr>,
        #[serde(default)]
        style: ArrayStyle,
    },
    Let { id: VarId, name: Symbol, value: Expr, body: Expr },
    Lambda {
        params: Vec<Symbol>,
        block_param: Option<Symbol>,
        body: Expr,
    },
    Apply { fun: Expr, args: Vec<Expr>, block: Option<Expr> },
    Send {
        /// `None` means implicit self (bare method call in current scope).
        recv: Option<Expr>,
        method: Symbol,
        args: Vec<Expr>,
        block: Option<Expr>,
        /// Did the source wrap args in parens (`foo(x)` vs `foo x`)? Matters
        /// only for implicit-self calls with args; explicit-receiver calls
        /// always use parens in Ruby syntax.
        #[serde(default)]
        parenthesized: bool,
    },
    If { cond: Expr, then_branch: Expr, else_branch: Expr },
    Case { scrutinee: Expr, arms: Vec<Arm> },
    Seq { exprs: Vec<Expr> },
    Assign { target: LValue, value: Expr },
    Yield { args: Vec<Expr> },
    Raise { value: Expr },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Literal {
    Nil,
    Bool { value: bool },
    Int { value: i64 },
    Float { value: f64 },
    Str { value: String },
    Sym { value: Symbol },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pattern {
    Wildcard,
    Bind { name: Symbol },
    Lit { value: Literal },
    Array { elems: Vec<Pattern>, rest: Option<Symbol> },
    Record { fields: Vec<(Symbol, Pattern)>, rest: bool },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LValue {
    Var { id: VarId, name: Symbol },
    Ivar { name: Symbol },
    Attr { recv: Expr, name: Symbol },
    Index { recv: Expr, index: Expr },
}
