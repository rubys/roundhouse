use serde::{Deserialize, Serialize};

use crate::expr::{Expr, Literal};
use crate::ident::{ClassId, Symbol, TableRef};

/// Target-independent relational algebra. Each emitter lowers this to its
/// native form: raw SQL, Ecto, Diesel, SQLAlchemy, etc. Capturing ActiveRecord
/// relations as algebra (not as method-call chains) is what makes the lowering
/// tractable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Query {
    From { table: TableRef, as_class: Option<ClassId> },
    Where { source: Box<Query>, predicate: Predicate },
    Join { left: Box<Query>, right: Box<Query>, join_kind: JoinKind, on: Predicate },
    Select { source: Box<Query>, columns: Vec<ColumnExpr> },
    OrderBy { source: Box<Query>, keys: Vec<OrderKey> },
    Limit { source: Box<Query>, count: u64 },
    Offset { source: Box<Query>, count: u64 },
    GroupBy { source: Box<Query>, columns: Vec<ColumnExpr> },
    Having { source: Box<Query>, predicate: Predicate },
    Distinct { source: Box<Query> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderKey {
    pub column: ColumnExpr,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Predicate {
    And { parts: Vec<Predicate> },
    Or { parts: Vec<Predicate> },
    Not { inner: Box<Predicate> },
    Eq { left: ColumnExpr, right: ValueExpr },
    Ne { left: ColumnExpr, right: ValueExpr },
    Lt { left: ColumnExpr, right: ValueExpr },
    Le { left: ColumnExpr, right: ValueExpr },
    Gt { left: ColumnExpr, right: ValueExpr },
    Ge { left: ColumnExpr, right: ValueExpr },
    In { left: ColumnExpr, values: Vec<ValueExpr> },
    Like { left: ColumnExpr, pattern: String },
    IsNull { column: ColumnExpr },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnExpr {
    pub table: Option<TableRef>,
    pub column: Symbol,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueExpr {
    Lit { value: Literal },
    Param { name: Symbol },
    Expr { expr: Expr },
}
