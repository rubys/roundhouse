//! Typed JS/TS AST the TypeScript emitter constructs instead of
//! strings. The printer (`printer.rs`) is the only place that renders
//! it — one walk produces source text and, when a mapping sink is
//! supplied, token-level source-map entries from the `Span` each node
//! carries (the originating Ruby/ERB position, exact since the real-
//! spans + ERB-offset-translation work).
//!
//! Design notes:
//! - Expressions (`Js`) and statements (`JsStmt`) are separate types,
//!   ESTree-style, so a statement can't appear in argument position.
//! - No `Paren` node: the printer derives parentheses from the
//!   precedence table. Constructors never pre-wrap.
//! - TS types are opaque leaves (`TsType`) rendered by `ty.rs` — they
//!   are synthesized (never user-source), need no source mapping, and
//!   keeping one Ty renderer avoids a second type grammar here.
//! - `span` is `Span::synthetic()` for glue the emitter invents;
//!   constructors that correspond to an IR node take the IR span.

use crate::span::Span;

/// Rendered TypeScript type, e.g. `string`, `Article[]`,
/// `Record<string, any>`. Produced by `ty.rs`; opaque here.
#[derive(Clone, Debug, PartialEq)]
pub struct TsType(pub String);

/// A JS/TS expression with the source span it was derived from.
#[derive(Clone, Debug, PartialEq)]
pub struct Js {
    pub span: Span,
    pub node: Box<JsExpr>,
}

/// A JS/TS statement with the source span it was derived from.
#[derive(Clone, Debug, PartialEq)]
pub struct JsStmt {
    pub span: Span,
    pub node: Box<JsStmtNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JsExpr {
    /// Identifier or keyword-expression: `articles`, `this`,
    /// `undefined`. The printer emits it verbatim.
    Ident(String),
    /// Numeric literal, already rendered: `42`, `3.5`.
    Num(String),
    /// String literal — the UNESCAPED text; the printer owns quoting
    /// and escaping (double quotes, `\n`-style escapes).
    Str(String),
    Bool(bool),
    Null,
    /// Template literal: `` `a${b}c` ``. Text parts are unescaped;
    /// the printer owns backtick escaping.
    Template(Vec<TplPart>),
    /// Regex literal: `/pattern/flags`, both verbatim.
    Regex { pattern: String, flags: String },
    Array(Vec<Js>),
    Object(Vec<(JsKey, Js)>),
    /// Arrow function. `body` is either a bare expression
    /// (`x => x + 1`) or a block (`x => { ... }`).
    Arrow {
        params: Vec<JsParam>,
        body: ArrowBody,
        is_async: bool,
    },
    Call { callee: Js, args: Vec<Js> },
    New { callee: Js, args: Vec<Js> },
    /// `obj.prop` — `prop` must be a valid identifier; use `Index`
    /// for computed access.
    Member { obj: Js, prop: String },
    /// `obj[index]`
    Index { obj: Js, index: Js },
    /// Binary operator from the fixed table in `printer::bin_prec`.
    Binary { op: &'static str, left: Js, right: Js },
    /// Prefix unary: `!`, `-`, `+`, `typeof `, `void `, `await `.
    /// (`await` as Unary keeps one precedence path; constructors
    /// expose `Js::await_` for readability.)
    Unary { op: &'static str, operand: Js },
    Ternary { cond: Js, then: Js, else_: Js },
    /// Assignment in expression position: `op` is `=`, `+=`, `??=`, ….
    Assign { target: Js, op: &'static str, value: Js },
    /// `(expr as T)` type assertion.
    Cast { expr: Js, ty: TsType },
    /// `...expr` in argument or array position.
    Spread(Js),
    /// Migration escape hatch: pre-rendered text spliced verbatim at
    /// expression precedence ATOM (never re-parenthesized). Deleted
    /// from the enum at switchover — nothing ships through it.
    Raw(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TplPart {
    /// Literal chunk (unescaped; printer escapes backticks/`${`).
    Text(String),
    /// `${expr}` interpolation.
    Expr(Js),
}

/// Object-literal key.
#[derive(Clone, Debug, PartialEq)]
pub enum JsKey {
    /// Bare identifier key: `article:`.
    Ident(String),
    /// Quoted string key: `"content_type":` (unescaped text).
    Str(String),
}

/// Function/method/arrow parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct JsParam {
    pub name: String,
    /// `name?: T` when optional.
    pub optional: bool,
    pub ty: Option<TsType>,
    pub default: Option<Js>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArrowBody {
    Expr(Js),
    Block(Vec<JsStmt>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum JsStmtNode {
    Expr(Js),
    /// `const`/`let` declaration. `ty` renders as `: T`.
    VarDecl {
        kind: VarKind,
        name: String,
        ty: Option<TsType>,
        init: Option<Js>,
    },
    Return(Option<Js>),
    If {
        cond: Js,
        then: Vec<JsStmt>,
        /// `None` = no else; `Some` holding a single `If` statement
        /// renders as `else if`.
        else_: Option<Vec<JsStmt>>,
    },
    While { cond: Js, body: Vec<JsStmt> },
    ForOf {
        binding: String,
        iterable: Js,
        body: Vec<JsStmt>,
    },
    Switch {
        scrutinee: Js,
        /// Each case body renders with a trailing `break;` unless it
        /// ends in `return`/`throw`.
        cases: Vec<(Js, Vec<JsStmt>)>,
        default: Option<Vec<JsStmt>>,
    },
    Throw(Js),
    Try {
        body: Vec<JsStmt>,
        /// `catch (binding) { ... }`; `None` binding = bare `catch {`.
        catch: Option<(Option<String>, Vec<JsStmt>)>,
        finally: Option<Vec<JsStmt>>,
    },
    Break,
    Continue,
    /// Standalone comment line(s): `// text` per entry.
    Comment(Vec<String>),
    /// Blank separator line between statement groups.
    Blank,
    /// Migration escape hatch — pre-rendered line(s) spliced at
    /// statement position. Deleted at switchover.
    Raw(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VarKind {
    Const,
    Let,
}

// ── module / declaration layer ───────────────────────────────────────

/// One emitted `.ts` module: header comment, imports, declarations.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct JsModule {
    /// `// ...` header lines (no slashes; printer adds them).
    pub header: Vec<String>,
    pub imports: Vec<JsImport>,
    pub decls: Vec<JsDecl>,
}

/// `import { name as alias, ... } from "from";`
#[derive(Clone, Debug, PartialEq)]
pub struct JsImport {
    pub names: Vec<(String, Option<String>)>,
    pub from: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JsDecl {
    Function {
        export: bool,
        is_async: bool,
        name: String,
        params: Vec<JsParam>,
        ret: Option<TsType>,
        body: Vec<JsStmt>,
        span: Span,
    },
    Class {
        export: bool,
        name: String,
        extends: Option<String>,
        members: Vec<JsClassMember>,
        span: Span,
    },
    /// `export const NAME = <expr>;` / `const NAME: T = <expr>;`
    Const {
        export: bool,
        name: String,
        ty: Option<TsType>,
        value: Js,
        span: Span,
    },
    /// `export type Name = <rendered>;`
    TypeAlias {
        export: bool,
        name: String,
        ty: TsType,
    },
    /// Migration escape hatch — a pre-rendered declaration block.
    /// Deleted at switchover.
    Raw(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum JsClassMember {
    /// Field declaration: `declare id: number;` / `body: string;` /
    /// `static table = "x";`
    Field {
        is_static: bool,
        declare: bool,
        name: String,
        ty: Option<TsType>,
        init: Option<Js>,
        span: Span,
    },
    Method {
        is_static: bool,
        is_async: bool,
        kind: MethodKind,
        name: String,
        params: Vec<JsParam>,
        ret: Option<TsType>,
        body: Vec<JsStmt>,
        span: Span,
    },
    /// Blank separator between member groups.
    Blank,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MethodKind {
    Normal,
    Get,
    Set,
    Constructor,
}

// ── constructors ─────────────────────────────────────────────────────
//
// Span-first, mirroring `Expr::new`. The handful of shapes the
// emitter builds constantly get dedicated helpers; everything else
// goes through `Js::new` / `JsStmt::new`.

impl Js {
    pub fn new(span: Span, node: JsExpr) -> Self {
        Js { span, node: Box::new(node) }
    }

    /// Synthesized glue with no source position of its own.
    pub fn synth(node: JsExpr) -> Self {
        Js::new(Span::synthetic(), node)
    }

    pub fn ident(span: Span, name: impl Into<String>) -> Self {
        Js::new(span, JsExpr::Ident(name.into()))
    }

    pub fn str(span: Span, text: impl Into<String>) -> Self {
        Js::new(span, JsExpr::Str(text.into()))
    }

    pub fn num(span: Span, text: impl Into<String>) -> Self {
        Js::new(span, JsExpr::Num(text.into()))
    }

    pub fn call(span: Span, callee: Js, args: Vec<Js>) -> Self {
        Js::new(span, JsExpr::Call { callee, args })
    }

    /// `obj.prop`
    pub fn member(span: Span, obj: Js, prop: impl Into<String>) -> Self {
        Js::new(span, JsExpr::Member { obj, prop: prop.into() })
    }

    /// `obj.method(args)` — the dominant emit shape.
    pub fn method_call(
        span: Span,
        obj: Js,
        method: impl Into<String>,
        args: Vec<Js>,
    ) -> Self {
        let callee = Js::member(span, obj, method);
        Js::call(span, callee, args)
    }

    pub fn index(span: Span, obj: Js, index: Js) -> Self {
        Js::new(span, JsExpr::Index { obj, index })
    }

    pub fn binary(span: Span, op: &'static str, left: Js, right: Js) -> Self {
        Js::new(span, JsExpr::Binary { op, left, right })
    }

    pub fn unary(span: Span, op: &'static str, operand: Js) -> Self {
        Js::new(span, JsExpr::Unary { op, operand })
    }

    pub fn await_(span: Span, operand: Js) -> Self {
        Js::unary(span, "await ", operand)
    }

    pub fn this() -> Self {
        Js::synth(JsExpr::Ident("this".into()))
    }
}

impl JsStmt {
    pub fn new(span: Span, node: JsStmtNode) -> Self {
        JsStmt { span, node: Box::new(node) }
    }

    pub fn synth(node: JsStmtNode) -> Self {
        JsStmt::new(Span::synthetic(), node)
    }

    pub fn expr(e: Js) -> Self {
        let span = e.span;
        JsStmt::new(span, JsStmtNode::Expr(e))
    }
}
