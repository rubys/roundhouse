//! Type ascriptions written in the source: `T.let(x, Type)` /
//! `T.cast(x, Type)` and the RBS inline forms `x = value #: Type` /
//! `x = value #: as Type`.
//!
//! All of them say, in the author's own words, what the value at that
//! spot is. Ingest used to throw the words away and keep the value
//! (sorbet-runtime's assertions evaluate to their first argument), so a
//! value the analyzer could not type stayed untyped even though the
//! source declared it — every read of `@preview_token` after
//! `@preview_token = T.let(result.ok_value, T.nilable(Token))` reported
//! `ivar_unresolved`. The declared type now rides along as an
//! [`ExprNode::Cast`], which the typer already reads as "this
//! expression has this type".

use std::cell::RefCell;

use ruby_prism::Node;

use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

use super::sorbet_sig::sorbet_type_node;
use super::sources;

thread_local! {
    /// End offsets of ancestors that will consume a trailing `#:` at that
    /// offset. A child ending at the same place must not also ascribe.
    static OUTER_ASCRIPTION_ENDS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Enter an expression that may claim a trailing `#:` at `end`. Returns
/// whether this call is the outermost claimant (no ancestor already
/// covers the same offset). Pair with [`pop_trailing_ascription_claim`].
pub(super) fn push_trailing_ascription_claim(end: usize) -> bool {
    OUTER_ASCRIPTION_ENDS.with(|stack| {
        let outermost = !stack.borrow().contains(&end);
        stack.borrow_mut().push(end);
        outermost
    })
}

pub(super) fn pop_trailing_ascription_claim() {
    OUTER_ASCRIPTION_ENDS.with(|stack| {
        stack.borrow_mut().pop();
    });
}

/// Wrap `value` in a `Cast` to `ty`, unless the type is unreadable
/// (`None` or an open inference variable), in which case `value` is
/// returned as is. `untyped` IS a reading: `T.unsafe(x)` and
/// `x #: as untyped` are the author's signed escape hatch, so the value
/// leaves the type system rather than keeping the type inferred for it.
pub(super) fn ascribe(value: Expr, ty: Option<Ty>) -> Expr {
    match ty {
        Some(ty) if !ty.is_open() => {
            let span = value.span;
            let mut cast = Expr::new(span, ExprNode::Cast { value, target_ty: ty });
            cast.decisions |= crate::expr::SOURCE_TYPE_ASCRIPTION;
            cast
        }
        _ => value,
    }
}

/// The declared type of a `T.let(x, Type)` / `T.cast(x, Type)` call.
pub(super) fn sorbet_declared_type(node: &Node<'_>) -> Option<Ty> {
    let call = node.as_call_node()?;
    let method = call.name();
    let method = std::str::from_utf8(method.as_slice()).ok()?;
    // `T.unsafe(x)` is the escape hatch: no second argument, the value
    // is untyped from there on.
    if method == "unsafe" {
        return Some(Ty::Untyped);
    }
    if !matches!(method, "let" | "cast") {
        return None;
    }
    let args = call.arguments()?.arguments();
    let ty = args.iter().nth(1)?;
    sorbet_type_node(&ty)
}

/// What a trailing `#:` comment on a line says about the expression it
/// follows.
pub(super) enum Trailing {
    /// `#: as Type` / `#: Type` — the value has this type.
    Type(Ty),
    /// `#: as !nil` — RBS inline's `T.must`: the value, with nil ruled out.
    NotNil,
}

/// The `#:` comment that follows the expression ending at byte `end`
/// on the same line, read as [`Trailing`]. A comma may sit between them:
/// the comment on an argument written one per line
/// (`delivery.lines.first, #: as !nil`) belongs to that argument.
pub(super) fn trailing_ascription(file: &str, end: usize) -> Option<Trailing> {
    // Only the comment text is copied out, and only when there is one.
    let comment = sources::with_text(file, |text| {
        let line = text.get(end..)?.split('\n').next()?.trim_start();
        let line = line.strip_prefix(',').map_or(line, str::trim_start);
        Some(line.strip_prefix("#:")?.trim().to_string())
    })??;
    let comment = comment.strip_prefix("as ").unwrap_or(&comment).trim();
    if comment == "!nil" {
        return Some(Trailing::NotNil);
    }
    rbs_type(comment).map(Trailing::Type)
}

/// Apply a trailing ascription to `value`: a `Cast` to the declared
/// type, or `value.not_nil!` for `#: as !nil` — the method core writes
/// instead of `T.must`, which the analyzer already reads as "the
/// receiver, nil removed".
pub(super) fn ascribe_trailing(value: Expr, trailing: Option<Trailing>) -> Expr {
    match trailing {
        Some(Trailing::Type(ty)) => ascribe(value, Some(ty)),
        Some(Trailing::NotNil) => not_nil(value),
        None => value,
    }
}

/// A generated non-nil assertion carries a refusal until nil-check semantics
/// have a shared executable implementation.
pub(super) fn not_nil(value: Expr) -> Expr {
    let span = value.span;
    let mut expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(value),
            method: crate::ident::Symbol::from("not_nil!"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    expr.diagnostic = Some(crate::diagnostic::DiagnosticKind::Unsupported {
        target: None,
        construct: crate::ident::Symbol::from("non-nil assertion"),
        detail: "requires a shared nil-check implementation".into(),
    });
    expr
}

/// The `#:` assertion written between a call's receiver and the
/// leading-dot line that continues the call:
///
/// ```text
/// self #: as untyped
///   .before_update(prepend: true) { ... }
/// ```
///
/// The comment ends the receiver's own line, so it is read exactly as
/// [`trailing_ascription`] reads one after any expression (`as T`,
/// `T`, `!nil`) -- but only when the next non-blank line continues with
/// `.` or `&.`; a comment after a receiver on the same line as its
/// method (`x.foo #: as T`) never gets this far, because the text after
/// the receiver is then the method, not the comment. Receivers that
/// read their own trailing comment in `ingest_expr_strict` (calls,
/// variable reads, parentheses) do not come here, so no assertion is
/// applied twice; this covers the rest (`self`, constants, literals).
pub(super) fn receiver_rbs_assertion(file: &str, end: usize) -> Option<Trailing> {
    let continues = sources::with_text(file, |text| {
        let rest = text.get(end..)?;
        let mut lines = rest.split('\n');
        lines.next()?.trim_start().strip_prefix("#:")?;
        Some(
            lines
                .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .is_some_and(|l| {
                    let l = l.trim_start();
                    l.starts_with('.') || l.starts_with("&.")
                }),
        )
    })??;
    if !continues {
        return None;
    }
    trailing_ascription(file, end)
}

/// Parse one RBS type expression by wrapping it in a method signature
/// the RBS reader already understands.
pub(crate) fn rbs_type(text: &str) -> Option<Ty> {
    if text.is_empty() || text.contains('\n') || text.starts_with('!') {
        return None;
    }
    let src = format!("class X\n  def m: () -> {text}\nend\n");
    let sigs = crate::rbs::parse_signatures(&src).ok()?;
    let (_, sig) = sigs.methods.into_iter().next()?;
    match sig {
        Ty::Fn { ret, .. } => Some(std::sync::Arc::unwrap_or_clone(ret)),
        _ => None,
    }
}
