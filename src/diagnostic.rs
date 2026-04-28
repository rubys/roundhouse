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
//! context. Newer kinds (`IncompatibleBinop`) are set directly by the
//! body-typer at the point of detection. Over time the walker-based
//! ones can migrate to annotations too; the visible output is the
//! same either way.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub span: Span,
    pub kind: DiagnosticKind,
    pub severity: Severity,
    pub message: String,
}

/// Severity gates whether a diagnostic counts as a build-stopping
/// error or a non-blocking warning. The default severity per kind is
/// chosen at construction (errors for analyzer-detected bugs,
/// warnings for author-signed gradual escapes); per-target emitters
/// may *elevate* a warning to an error at emit time when the target
/// can't accept the gradual escape (Rust elevating `GradualUntyped`
/// is the canonical case — see `ty.rs::Ty::Untyped`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

impl Diagnostic {
    /// Short identifier for this diagnostic kind — a grep-friendly
    /// code the user can search for (`ivar_unresolved`,
    /// `send_dispatch_failed`, `incompatible_binop`,
    /// `gradual_untyped`).
    pub fn code(&self) -> &'static str {
        match self.kind {
            DiagnosticKind::IvarUnresolved { .. } => "ivar_unresolved",
            DiagnosticKind::SendDispatchFailed { .. } => "send_dispatch_failed",
            DiagnosticKind::IncompatibleBinop { .. } => "incompatible_binop",
            DiagnosticKind::GradualUntyped { .. } => "gradual_untyped",
        }
    }

    /// Default severity for a kind. Most kinds are user-error-level
    /// (Error). `GradualUntyped` is the explicit author-signed
    /// gradual escape; default Warning, with strict-target emitters
    /// elevating to Error at emit time.
    pub fn default_severity(kind: &DiagnosticKind) -> Severity {
        match kind {
            DiagnosticKind::GradualUntyped { .. } => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

impl fmt::Display for Diagnostic {
    /// Single-line rendering. With current Span infrastructure we
    /// have no usable file:line info, so the rendering is message-
    /// only; identifier names in the message (method, ivar, types)
    /// are the user's grep targets until real spans land.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(f, "{prefix}[{}]: {}", self.code(), self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::ClassId;

    #[test]
    fn display_renders_ivar_unresolved() {
        let d = Diagnostic {
            span: Span::synthetic(),
            kind: DiagnosticKind::IvarUnresolved {
                name: Symbol::from("article"),
            },
            severity: Severity::Error,
            message: "@article has no known type".to_string(),
        };
        assert_eq!(
            d.to_string(),
            "error[ivar_unresolved]: @article has no known type"
        );
    }

    #[test]
    fn display_renders_send_dispatch_failed() {
        let d = Diagnostic {
            span: Span::synthetic(),
            kind: DiagnosticKind::SendDispatchFailed {
                method: Symbol::from("frobnicate"),
                recv_ty: Ty::Class {
                    id: ClassId(Symbol::from("Article")),
                    args: vec![],
                },
            },
            severity: Severity::Error,
            message: "no known method `frobnicate` on Class(Article)".to_string(),
        };
        assert!(d.to_string().starts_with("error[send_dispatch_failed]: "));
        assert!(d.to_string().contains("frobnicate"));
    }

    #[test]
    fn display_renders_incompatible_binop() {
        let d = Diagnostic {
            span: Span::synthetic(),
            kind: DiagnosticKind::IncompatibleBinop {
                op: Symbol::from("+"),
                lhs_ty: Ty::Int,
                rhs_ty: Ty::Str,
            },
            severity: Severity::Error,
            message: "`+` with incompatible operand types: Int + Str".to_string(),
        };
        assert_eq!(
            d.to_string(),
            "error[incompatible_binop]: `+` with incompatible operand types: Int + Str"
        );
    }
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
    /// `a OP b` with concrete operand types that Ruby would reject
    /// at runtime — `Int + Str`, `Hash + Hash`, `1 < "hello"`, etc.
    /// `op` is the Ruby method symbol (`+`, `<`, `==`, …) so a
    /// single variant covers every binary operator; the message
    /// formatter uses it to name the operator for the user. The
    /// emitter produces a target-side raise-equivalent at the site
    /// so the compiled program preserves Ruby's runtime behavior.
    /// Annotated directly on the Send Expr by the body-typer.
    IncompatibleBinop {
        op: Symbol,
        lhs_ty: Ty,
        rhs_ty: Ty,
    },
    /// An expression's type resolved to RBS-declared `untyped`
    /// (`Ty::Untyped`) — the gradual-typing escape hatch was reached
    /// at this site. Default severity is Warning: dynamic-target
    /// emitters (TS `any`, Python `Any`, Elixir dynamic) accept
    /// this; strict-target emitters (Rust, Go) are expected to
    /// elevate it to Error at emit time. The `expr_kind` field
    /// captures what kind of node carried the `Untyped` so a single
    /// variant can name "method receiver", "argument", "return",
    /// etc., for downstream rendering.
    GradualUntyped { expr_kind: Symbol },
}
