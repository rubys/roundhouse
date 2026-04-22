//! Roundhouse — Rails is the specification, deployment is a build flag.
//!
//! Defines the Roundhouse intermediate representation: a typed, effect-tracked,
//! serializable core with a Rails dialect layered on top. The IR is the
//! deliverable; ingesters produce it, emitters consume it.

pub mod adapter;
pub mod analyze;
pub mod app;
pub mod bytecode;
pub mod catalog;
pub mod dialect;
pub mod effect;
pub mod emit;
pub mod erb;
pub mod expr;
pub mod ident;
pub mod ingest;
pub mod lower;
pub mod naming;
pub mod query;
pub mod rbs;
pub mod runtime_src;
pub mod schema;
pub mod span;
pub mod ty;

pub use adapter::{ArMethodKind, DatabaseAdapter, SqliteAdapter, SqliteAsyncAdapter};
pub use app::App;
pub use dialect::{
    Action, Association, Callback, CallbackHook, Comment, Controller, ControllerBodyItem,
    Dependent, Filter, FilterKind, HttpMethod, MethodDef, MethodReceiver, Model, ModelBodyItem,
    RenderTarget, Route, RouteSpec, RouteTable, Scope, Validation, ValidationRule, View,
};
pub use effect::{Effect, EffectSet};
pub use expr::{Arm, BlockStyle, Expr, ExprNode, LValue, Literal, Pattern};
pub use ident::{ClassId, EffectVar, Symbol, TableRef, TyVar, VarId};
pub use query::{ColumnExpr, JoinKind, OrderKey, Predicate, Query, ValueExpr};
pub use schema::{Column, ColumnType, ForeignKey, Index, ReferentialAction, Schema, Table};
pub use span::{FileId, Span};
pub use ty::{Param, ParamKind, Row, Ty};
