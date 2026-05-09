//! Arel IR — query algebra evaluable at transpile time.
//!
//! Most real Rails AR usage is statically resolvable: associations have
//! known schema; named scopes have literal bodies; `where`/`find`/`count`
//! arguments are typically literals or single-runtime-value substitutions.
//! That majority becomes an `ArelOp` tree whose per-backend visitor
//! (see `visitor.rs`) emits inline SQL composition + per-model hydration
//! at codegen time. The dynamic minority (conditional chains, dynamic
//! hashes) is Phase 2's concern: a runtime Arel module in framework
//! Ruby with the same shape.
//!
//! Design notes:
//!
//! - `Value::Runtime { expr, ty }` carries the unrendered Expr that
//!   provides the value at runtime, plus the `ValueType` the visitor
//!   needs to pick `escape_int` vs `escape_string`. The visitor wraps
//!   the expr in `Db.escape_<ty>(<expr>)`.
//!
//! - `ColRef` is just (table, col); joins (Phase 3+) will need an alias
//!   slot — reserved by leaving the field name alone for now.
//!
//! - `LimitSpec` is `u64` only (literal). Runtime limits (pagination)
//!   can extend later; the per-shape adapter methods we replace today
//!   only ever pass literal limits.
//!
//! - Insert and Update share the `Assignment` shape: ordered, with
//!   `Value` on the right-hand side. The schema-derived column order
//!   matters for emit (the SELECT cols list must match the hydration
//!   order); Insert preserves it via `Vec<Assignment>` in schema order.
//!
//! See `project_arel_compile_time_first.md` for the broader architecture.

use crate::expr::Expr;
use crate::ident::{Symbol, TableRef};

/// Top-level Arel operation. One per call-site recognized by
/// `try_build_arel`.
#[derive(Clone, Debug)]
pub enum ArelOp {
    Select(Select),
    Insert(Insert),
    Update(Update),
    Delete(Delete),
}

/// `SELECT … FROM … [WHERE …] [ORDER BY …] [LIMIT …]`.
///
/// `columns` covers the common shapes:
/// - `All` — every schema column in declaration order; the visitor
///   expands at visit time and matches the hydration order.
/// - `Named` — explicit list (reserved; not used by the 8 starting
///   shapes, but find_by(<col>) shapes will want it).
/// - `Count` — `SELECT COUNT(*)`; visitor returns the count expr.
#[derive(Clone, Debug)]
pub struct Select {
    pub table: TableRef,
    pub columns: ColumnSpec,
    pub conditions: Option<Predicate>,
    pub orders: Vec<Order>,
    pub limit: Option<LimitSpec>,
    /// Reserved for joins / includes / preload — Phase 3+. Empty for
    /// every Select built today; visitor ignores.
    pub joins: Vec<Join>,
}

/// `INSERT INTO <table> (<cols>) VALUES (<values>)`.
///
/// `assignments` is in schema column order minus the primary key
/// (matching today's `_adapter_insert` shape). Visitor composes
/// CSV cols + escaped values; returns the Expr that calls
/// `Db.exec(sql)` then `Db.last_insert_rowid`.
#[derive(Clone, Debug)]
pub struct Insert {
    pub table: TableRef,
    pub assignments: Vec<Assignment>,
}

/// `UPDATE <table> SET <col = val>, … WHERE <conditions>`.
///
/// `conditions` is required for the `_adapter_update(id, instance)`
/// shape (Eq(id, Runtime(id))). A future bulk-update would allow None.
#[derive(Clone, Debug)]
pub struct Update {
    pub table: TableRef,
    pub assignments: Vec<Assignment>,
    pub conditions: Option<Predicate>,
}

/// `DELETE FROM <table> [WHERE <conditions>]`.
///
/// `conditions` None covers `_adapter_truncate` (test setup);
/// `Some(Eq(id, Runtime(id)))` covers `_adapter_delete(id)`.
#[derive(Clone, Debug)]
pub struct Delete {
    pub table: TableRef,
    pub conditions: Option<Predicate>,
}

/// One column-equals-value pair on the LHS of an Insert/Update.
/// Visitor renders as `<col> = <escaped value>` (Update) or
/// contributes `<col>` to the cols-list and `<escaped value>` to the
/// values-list (Insert).
#[derive(Clone, Debug)]
pub struct Assignment {
    pub column: Symbol,
    pub value: Value,
}

/// What a Select projects, and (implicitly) what shape the visitor
/// materializes at the boundary.
///
/// - `All` + `limit Some(1)` → nilable instance (single-row hydrate)
/// - `All` + no limit         → array of instances (loop hydrate)
/// - `Count`                  → integer scalar
/// - `Exists`                 → bool from `step?` (projection is the
///                              sentinel `1`; column reads skipped)
#[derive(Clone, Debug)]
pub enum ColumnSpec {
    /// Every schema column in declaration order. Visitor expands at
    /// visit time so the hydrate path knows the expected order.
    All,
    /// Reserved — explicit projection (e.g. `SELECT id, title`).
    /// Not built by Phase 1 patterns; landed here so the find_by
    /// extension can target it later without a schema bump.
    Named(Vec<ColRef>),
    /// `SELECT COUNT(*)`. Visitor returns the integer scalar.
    Count,
    /// `SELECT 1` projection whose result is consumed only by
    /// `step?` — the visitor emits a Bool-returning body. Models the
    /// `_adapter_exists_by_id?` shape and `Model.exists?(...)` at
    /// runtime call sites.
    Exists,
}

/// Boolean expression in the WHERE position. Phase 1 is small on
/// purpose: Eq is what the 8 adapter methods need and what every
/// has_many proxy emits. Other variants will land as their fixtures
/// surface (Neq for negation scopes, In for batched lookups, …).
#[derive(Clone, Debug)]
pub enum Predicate {
    Eq(ColRef, Value),
    And(Box<Predicate>, Box<Predicate>),
    Or(Box<Predicate>, Box<Predicate>),
}

/// Reference to a column. Just (table, col) for now; joins will
/// promote this to a qualified form (alias.col) without disturbing
/// existing call sites — the (table, col) pair is already the
/// fully-qualified shape under a single-table SELECT.
#[derive(Clone, Debug)]
pub struct ColRef {
    pub table: TableRef,
    pub column: Symbol,
}

/// A value that flows into a SQL string at SQL composition time.
/// `Literal*` variants are baked into the SQL at codegen (bare).
/// `Runtime { expr, ty }` defers value computation to the emitted
/// program: visitor wraps `expr` in `Db.escape_<ty>(<expr>)` and
/// composes the result into the SQL string.
#[derive(Clone, Debug)]
pub enum Value {
    LiteralInt(i64),
    LiteralStr(String),
    LiteralBool(bool),
    LiteralNull,
    /// Visitor wraps this expression in `Db.escape_<ty>(<expr>)`. The
    /// expression is itself an unrendered subtree of the source
    /// program — typically `id` (Var ref to a method param) or
    /// `instance.<col>` (instance-field accessor) or `@id` (Ivar).
    Runtime { expr: Expr, ty: ValueType },
}

/// Discriminator for which `Db.escape_*` primitive the visitor calls.
/// One per primitive runtime supports today; Float/Bool can land when
/// the runtime grows them. Mirrors the column-type → escape-method
/// branch in today's `adapter_emit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    Int,
    Str,
    /// Bool not yet supported by the runtime escape surface; reserved.
    Bool,
}

/// `ORDER BY <col> ASC|DESC`. Reserved; no current call site builds
/// orders, but the runtime-Arel chain will (`Article.all.order(:created_at)`).
#[derive(Clone, Debug)]
pub struct Order {
    pub column: ColRef,
    pub direction: Direction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Asc,
    Desc,
}

/// `LIMIT n`. Literal-only for now; runtime-driven pagination would
/// extend this with a `Runtime(Expr)` variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitSpec(pub u64);

/// Reserved for Phase 3+. Joins/includes/preload all land in this
/// slot once their fixtures surface; today every Select carries an
/// empty `joins: Vec<Join>` and the visitor ignores.
#[derive(Clone, Debug)]
pub struct Join {
    pub table: TableRef,
    pub kind: JoinKind,
    pub on: Predicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
}
