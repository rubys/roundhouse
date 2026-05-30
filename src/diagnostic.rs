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
            DiagnosticKind::Unsupported { .. } => "unsupported",
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

    /// Construct an `Unsupported` diagnostic for a tool-coverage gap.
    /// The span is synthetic (real spans are a separate, stubbed gap);
    /// severity is the kind default (`Error`); the message is the
    /// canonical [`Self::unsupported_text`] plus any `detail`.
    ///
    /// `target` is `None` for target-agnostic gaps (e.g. a shared
    /// lowerer). `construct` and `detail` accept anything `Into<…>` so
    /// call sites can pass `&str`/`String`/`Symbol` without ceremony.
    pub fn unsupported(
        target: Option<Symbol>,
        construct: impl Into<Symbol>,
        detail: impl Into<String>,
    ) -> Self {
        let construct = construct.into();
        let detail = detail.into();
        let mut message = Self::unsupported_text(target.as_ref(), &construct);
        if !detail.is_empty() {
            message.push_str(": ");
            message.push_str(&detail);
        }
        let kind = DiagnosticKind::Unsupported { target, construct, detail };
        Diagnostic {
            span: Span::synthetic(),
            severity: Self::default_severity(&kind),
            kind,
            message,
        }
    }

    /// Short human text for a diagnostic *kind*, used to render the
    /// runtime raise/panic/throw stub an emitter drops at a site
    /// carrying `Expr.diagnostic` (via [`crate::emit::diagnostics::StubStyle::render`]).
    /// Unlike the per-`Diagnostic` `message`, this is reconstructable
    /// from the kind alone — so the emit short-circuit names the actual
    /// gap (`While`, `ColumnSpec::Named`) instead of a hardcoded
    /// operator. Kept terse; the full detail lives on the collected
    /// `Diagnostic`.
    pub fn stub_text(kind: &DiagnosticKind) -> String {
        match kind {
            DiagnosticKind::IncompatibleBinop { op, .. } => {
                format!("`{}` with incompatible operand types", op.as_str())
            }
            DiagnosticKind::IvarUnresolved { name } => {
                format!("@{} has no known type", name.as_str())
            }
            DiagnosticKind::SendDispatchFailed { method, .. } => {
                format!("no known method `{}`", method.as_str())
            }
            DiagnosticKind::GradualUntyped { expr_kind } => {
                format!("{} resolves to untyped", expr_kind.as_str())
            }
            DiagnosticKind::Unsupported { target, construct, .. } => {
                Self::unsupported_text(target.as_ref(), construct)
            }
        }
    }

    /// Canonical human text for an unsupported construct, shared between
    /// the diagnostic `message` and the `raise`/`panic` stub an emitter
    /// drops at the site — so the collected report and the compiled
    /// program name the gap identically. Renders `"<construct> not
    /// supported (<target>)"`, or `"… (all targets)"` when target-
    /// agnostic.
    pub fn unsupported_text(target: Option<&Symbol>, construct: &Symbol) -> String {
        let where_ = match target {
            Some(t) => t.as_str(),
            None => "all targets",
        };
        format!("{construct} not supported ({where_})")
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

    #[test]
    fn unsupported_constructor_targeted_with_detail() {
        let d = Diagnostic::unsupported(
            Some(Symbol::from("go")),
            "While",
            "loop body has non-tail statement",
        );
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.code(), "unsupported");
        assert_eq!(
            d.to_string(),
            "error[unsupported]: While not supported (go): loop body has non-tail statement"
        );
    }

    #[test]
    fn unsupported_constructor_target_agnostic_no_detail() {
        let d = Diagnostic::unsupported(None, "ColumnSpec::Named", "");
        assert_eq!(
            d.to_string(),
            "error[unsupported]: ColumnSpec::Named not supported (all targets)"
        );
    }

    #[test]
    fn stub_text_names_the_actual_kind() {
        // IncompatibleBinop names the real operator, not a hardcoded `+`.
        assert_eq!(
            Diagnostic::stub_text(&DiagnosticKind::IncompatibleBinop {
                op: Symbol::from("-"),
                lhs_ty: Ty::Int,
                rhs_ty: Ty::Str,
            }),
            "`-` with incompatible operand types"
        );
        // Unsupported renders the construct, reusing the canonical text.
        assert_eq!(
            Diagnostic::stub_text(&DiagnosticKind::Unsupported {
                target: None,
                construct: Symbol::from("ColumnSpec::Named"),
                detail: "ignored in the terse stub".to_string(),
            }),
            "ColumnSpec::Named not supported (all targets)"
        );
    }

    #[test]
    fn unsupported_text_is_shared_canonical_form() {
        let construct = Symbol::from("While");
        let target = Symbol::from("rust");
        assert_eq!(
            Diagnostic::unsupported_text(Some(&target), &construct),
            "While not supported (rust)"
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
    /// Valid Ruby the tool can't compile *yet* — a coverage gap in a
    /// lowerer or emitter, distinct from `IncompatibleBinop` (where the
    /// *source* is wrong and Ruby itself would raise). Default severity
    /// is `Error` (the output isn't a faithful compile), but it is
    /// **collected, not fatal**: the producing site emits a `raise`
    /// stub at that one location and lets the rest of the app
    /// transpile, so a single run yields the whole inventory of gaps.
    ///
    /// `target` names the backend that couldn't emit the construct
    /// (`None` when the gap is target-agnostic, e.g. produced by a
    /// shared lowerer). `construct` is a stable, grep-able identifier
    /// for what wasn't handled (an IR node kind, a method name). `detail`
    /// carries free-form context (the offending class, an inner error).
    Unsupported {
        target: Option<Symbol>,
        construct: Symbol,
        detail: String,
    },
}
