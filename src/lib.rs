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
pub mod diagnostic;
pub mod dialect;
pub mod effect;
pub mod emit;
pub mod erb;
pub mod expr;
pub mod ide;
pub mod ident;
pub mod ingest;
pub mod lower;
/// Standalone read-only LSP server over the [`ide`] query layer. Host-only
/// (uses stdio + the synchronous `lsp-server` transport); excluded from the
/// wasm build.
#[cfg(not(target_arch = "wasm32"))]
pub mod lsp;
pub mod naming;
pub mod profile;
#[cfg(not(target_arch = "wasm32"))]
pub mod project;
pub mod query;
pub mod rbs;
pub mod runtime_loader;
pub mod runtime_src;
pub mod treeshake;
pub mod schema;
pub mod span;
pub mod ty;
pub mod vfs;

pub use adapter::{ArMethodKind, DatabaseAdapter, SqliteAdapter, SqliteAsyncAdapter};
pub use app::App;
pub use profile::{Database, DeploymentProfile, HttpShim, ProfileError, Target};
pub use dialect::{
    Action, Association, Callback, CallbackHook, Comment, Controller, ControllerBodyItem,
    Dependent, Filter, FilterKind, HttpMethod, LayoutDecl, MethodDef, MethodReceiver, Model,
    ModelBodyItem,
    RenderTarget, Route, RouteSpec, RouteTable, Scope, Validation, ValidationRule, View,
};
pub use effect::{Effect, EffectSet};
pub use expr::{Arm, BlockStyle, Expr, ExprNode, LValue, Literal, Pattern};
pub use ident::{ClassId, EffectVar, Symbol, TableRef, TyVar, VarId};
pub use ide::{Position, TypeAt};
pub use query::{ColumnExpr, JoinKind, OrderKey, Predicate, Query, ValueExpr};
pub use schema::{Column, ColumnType, ForeignKey, Index, ReferentialAction, Schema, Table};
pub use span::{FileId, Span};
pub use ty::{Param, ParamKind, Row, Ty};
