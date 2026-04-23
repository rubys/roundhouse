//! Structured diagnostics carried with the IR.
//!
//! A [`DiagnosticKind`] is an annotation set on an [`crate::expr::Expr`]
//! (via its `diagnostic` field) or produced by a post-analyze walker
//! (via [`crate::analyze::diagnose`]). Both paths funnel into the same
//! [`Diagnostic`] shape so a future renderer (likely miette) has one
//! type to format.
//!
//! The split between "annotation on Expr" and "walker-produced" is
//! historical: older kinds (`IvarUnresolved`, `SendDispatchFailed`)
//! are produced by traversing the analyzed tree and inferring from
//! context. Newer kinds (`IncompatibleAdd`) are set directly by the
//! body-typer at the point of detection. Over time the walker-based
//! ones can migrate to annotations too; the visible output is the
//! same either way.

use serde::{Deserialize, Serialize};

use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub span: Span,
    pub kind: DiagnosticKind,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagnosticKind {
    /// `@name` read at a site where no action seeded the ivar — the
    /// controller→view channel (or before_action flow) didn't bind it.
    /// Produced by the walker in `analyze::diagnose`.
    IvarUnresolved { name: Symbol },
    /// `recv.method(...)` where `recv` has a known type but the method
    /// isn't in the registry for that type. Produced by the walker
    /// in `analyze::diagnose`.
    SendDispatchFailed { method: Symbol, recv_ty: Ty },
    /// `a + b` with concrete operand types that Ruby would reject at
    /// runtime (`Int + Str`, `Hash + Hash`, etc.). Ruby raises
    /// `TypeError` on evaluation; the emitter produces a target-side
    /// raise-equivalent so the compiled program preserves that
    /// behavior. Annotated directly on the Send Expr by the body-typer.
    IncompatibleAdd { lhs_ty: Ty, rhs_ty: Ty },
}
