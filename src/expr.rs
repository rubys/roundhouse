use serde::{Deserialize, Serialize};

use crate::effect::EffectSet;
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
    /// Side-effects this expression may perform. Populated by the analyzer
    /// during the same pass that assigns `ty`; ingest leaves it empty.
    /// Set semantics — the effects this node contributes *locally* (direct
    /// Sends on Active Record methods, `render`/`redirect_to` I/O, etc.);
    /// effects of nested subexpressions live on those subexpressions.
    /// Readers that want the transitive effect of a subtree can fold over
    /// the walk (same shape as the per-action aggregation in `analyze`).
    #[serde(default, skip_serializing_if = "EffectSet::is_pure")]
    pub effects: EffectSet,
    /// Set when this Expr is a `Seq` member whose source was preceded
    /// by a blank line. Meaningless outside that context; emit honors
    /// it when walking a `Seq` body. Populated from source offsets
    /// at ingest time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub leading_blank_line: bool,
}

impl Expr {
    pub fn new(span: Span, node: ExprNode) -> Self {
        Self {
            span,
            node: Box::new(node),
            ty: None,
            effects: EffectSet::pure(),
            leading_blank_line: false,
        }
    }
}

fn default_true() -> bool { true }

/// Surface form of an array literal. Source fidelity: `[:a, :b]` (Brackets),
/// `%i[a b]` (PercentI, symbol list), `%w[a b]` (PercentW, word list) all
/// produce the same Prism `ArrayNode` but differ byte-for-byte in source.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ArrayStyle {
    /// `[elem, elem, ...]` — the common form.
    #[default]
    Brackets,
    /// `[ elem, elem, ... ]` — brackets with a space between each
    /// bracket and the first / last element. Rails scaffolds emit
    /// literals this way in a few places (e.g. `params.expect(article:
    /// [ :title, :body ])`). Round-trip only; semantically identical
    /// to `Brackets`.
    BracketsSpaced,
    /// `%i[sym sym ...]` — symbol-list literal. Elements must be bare symbols.
    PercentI,
    /// `%w[word word ...]` — word-list literal. Elements must be bare strings.
    PercentW,
}

/// Delimiter style for a block body.
///
/// Ruby's two block forms (`{ … }` and `do … end`) bind differently to
/// chained method calls — `{ … }` binds tight, `do … end` binds to the
/// leftmost call. That difference is surface-observable and sometimes
/// semantically relevant, so we preserve whichever one the source used.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlockStyle {
    /// `do … end` (or no explicit delimiter context, e.g. lambda bodies
    /// that emit as `->(x) { … }` where the brace is implicit in the
    /// lambda form). The conservative default when style can't be
    /// determined.
    #[default]
    Do,
    /// `{ … }` — the tight-binding form; preferred for one-liners.
    Brace,
}

/// Which short-circuit operator is meant.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoolOpKind {
    And,
    Or,
}

/// Surface spelling for `BoolOp`. Ruby's `and`/`or` keywords have lower
/// precedence than `=` whereas `&&`/`||` bind tighter — not interchangeable
/// in all positions, so we preserve which one the source wrote.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BoolOpSurface {
    /// `&&` / `||` — the tight-binding operator form.
    #[default]
    Symbol,
    /// `and` / `or` — the keyword form (lower precedence).
    Word,
}

/// Piece of an interpolated string. Ingested from Prism's
/// InterpolatedStringNode so the emitter can re-synthesize `"x#{expr}y"`
/// byte-for-byte. Lowering to `"x" + expr.to_s + "y"` would lose the
/// distinction between real interpolation and real concatenation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InterpPart {
    /// Literal chunk between interpolations (already unescaped).
    Text { value: String },
    /// Embedded `#{expr}` — the expression's result is converted to a
    /// string at runtime.
    Expr { expr: Expr },
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
    /// Interpolated double-quoted string: `"x#{expr}y"`. Parts alternate
    /// between literal text and embedded expressions. A single-part
    /// Text-only list would degenerate to `Lit::Str` at ingest; we keep
    /// this variant reserved for cases with at least one Expr part.
    StringInterp { parts: Vec<InterpPart> },
    /// Short-circuit logical operator: `left && right` or `left || right`.
    /// Ruby also has keyword forms (`and`/`or`) with different precedence;
    /// `surface` preserves which spelling the source used so round-trip
    /// is byte-accurate.
    BoolOp {
        op: BoolOpKind,
        #[serde(default)]
        surface: BoolOpSurface,
        left: Expr,
        right: Expr,
    },
    Let { id: VarId, name: Symbol, value: Expr, body: Expr },
    Lambda {
        params: Vec<Symbol>,
        block_param: Option<Symbol>,
        body: Expr,
        /// Surface form when this Lambda represents a block attached to
        /// a method call (`foo { ... }` vs `foo do ... end`) — or the
        /// body delimiter of a `->` lambda (which is always braces in
        /// Prism, so we default to `Brace` for lambda literals).
        /// For round-trip fidelity; analyzer and typed targets ignore it.
        #[serde(default)]
        block_style: BlockStyle,
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
    /// Trailing `rescue` modifier: `expr rescue fallback`. Semantically
    /// `begin; expr; rescue StandardError; fallback; end` but preserved
    /// as its surface form so the Ruby emitter can round-trip it
    /// without promoting it to a multi-line `begin` block.
    RescueModifier { expr: Expr, fallback: Expr },
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
