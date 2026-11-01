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
//!
//! `T.bind(self, Type)` and its RBS inline form, a `#: self as Type`
//! line, say what `self` is for the rest of the enclosing body. Both
//! ingest as a statement casting `self`, marked
//! [`SELF_BINDING`](crate::expr::SELF_BINDING), which the typer reads
//! as the new type of `self`.

use std::cell::RefCell;

use ruby_prism::Node;
use super::prism_api::PrismLocation;

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

/// The declared type of a `T.let(x, Type)` / `T.cast(x, Type)` /
/// `T.bind(self, Type)` call.
pub(super) fn sorbet_declared_type(node: &Node<'_>) -> Option<Ty> {
    let call = node.as_call_node()?;
    let method = call.name();
    let method = std::str::from_utf8(method.as_slice()).ok()?;
    // `T.unsafe(x)` is the escape hatch: no second argument, the value
    // is untyped from there on.
    if method == "unsafe" {
        return Some(Ty::gradual_untyped());
    }
    if !matches!(method, "let" | "cast" | "bind") {
        return None;
    }
    let args = call.arguments()?.arguments();
    let ty = args.iter().nth(1)?;
    sorbet_type_node(&ty)
}

/// Mark `expr`, when it is a `Cast` of `self`, as the
/// [`SELF_BINDING`](crate::expr::SELF_BINDING) a `T.bind(self, Type)` or
/// `#: self as Type` is; `class_object` when the type is a class object
/// (`T.class_of(X)`, `singleton(X)`). A `self` left bare (its type
/// unreadable) binds nothing and is returned as is.
pub(super) fn bind_self(mut expr: Expr, class_object: bool) -> Expr {
    if matches!(&*expr.node, ExprNode::Cast { value, .. } if matches!(&*value.node, ExprNode::SelfRef)) {
        expr.decisions |= crate::expr::SELF_BINDING;
        if class_object {
            expr.decisions |= crate::expr::SELF_BINDING_CLASS_OBJECT;
        }
    }
    expr
}

/// Whether the type a `T.bind(self, Type)` call declares is a class
/// object, `T.class_of(X)`.
pub(super) fn sorbet_binds_class_object(node: &Node<'_>) -> bool {
    node.as_call_node()
        .and_then(|call| call.arguments()?.arguments().iter().nth(1))
        .and_then(|ty| ty.as_call_node())
        .is_some_and(|ty| {
            ty.name().as_slice() == b"class_of"
                && ty.receiver().is_some_and(|r| r.as_constant_read_node().is_some_and(|c| c.name().as_slice() == b"T"))
        })
}

/// The `#: self as Type` comment directly above the statement starting
/// at byte `start`, as the self binding it declares, spanning the
/// comment. RBS inline's spelling of `T.bind(self, Type)`: written on
/// its own line, it gives `self` that type from the statement below to
/// the end of the enclosing body. Only blank and comment lines may sit
/// between the two, and the statement must begin its line. `None` when
/// there is no such comment or its type is unreadable.
pub(super) fn leading_self_binding(file: &str, start: usize) -> Option<Expr> {
    let (at, len, written) = sources::with_text(file, |text| {
        let before = text.get(..start)?;
        let mut line_start = before.rfind('\n').map_or(0, |i| i + 1);
        if !before[line_start..].trim().is_empty() {
            return None;
        }
        while line_start > 0 {
            let above = &before[..line_start - 1];
            let above_start = above.rfind('\n').map_or(0, |i| i + 1);
            let line = &above[above_start..];
            let comment = line.trim();
            if let Some(rest) = comment.strip_prefix("#:") {
                let written = rest.trim().strip_prefix("self ").and_then(|r| r.trim_start().strip_prefix("as "));
                if let Some(written) = written {
                    let at = above_start + (line.len() - line.trim_start().len());
                    return Some((at, comment.len(), written.trim().to_string()));
                }
            } else if !comment.is_empty() && !comment.starts_with('#') {
                return None;
            }
            line_start = above_start;
        }
        None
    })??;
    let span = crate::span::Span {
        file: sources::file_id(file),
        start: at as u32,
        end: (at + len) as u32,
    };
    let class_object = written.starts_with("singleton(");
    let ty = rbs_type_with_vars(&written, &type_vars_at(file, start));
    let binding = bind_self(ascribe(Expr::new(span, ExprNode::SelfRef), ty), class_object);
    crate::expr::is_self_binding(&binding).then_some(binding)
}

/// Whether the statement list of `len` statements ending at byte `end` is
/// the body of a modifier (`stmt if cond`, `stmt while cond`): a single
/// statement followed on its line by the modifier's keyword. Such a body
/// begins where the statement it belongs to begins, so a comment above
/// that statement is read there, not again inside it.
pub(super) fn is_modifier_body(file: &str, len: usize, end: usize) -> bool {
    len == 1
        && sources::with_text(file, |text| {
            let rest = text.get(end..)?.split('\n').next()?.trim_start();
            Some(["if", "unless", "while", "until"].iter().any(|keyword| {
                rest.strip_prefix(keyword)
                    .is_some_and(|after| after.starts_with(|c: char| c.is_whitespace() || c == '(' || c == '!'))
            }))
        })
        .flatten()
        .unwrap_or(false)
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
    rbs_type_with_vars(comment, &type_vars_at(file, end)).map(Trailing::Type)
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
    expr.set_diagnostic(Some(crate::diagnostic::DiagnosticKind::Unsupported {
        target: None,
        construct: crate::ident::Symbol::from("non-nil assertion"),
        detail: "requires a shared nil-check implementation".into(),
    }));
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
/// the RBS reader already understands. The type variables in scope
/// (`vars`) are declared on that method, so `U` in `result = nil #: U?`
/// inside a `#: [U] (...) -> U` method reads as a type variable
/// (untyped, as in the signature itself), not a constant named `U`.
fn rbs_type_with_vars(text: &str, vars: &[String]) -> Option<Ty> {
    if text.is_empty() || text.contains('\n') || text.starts_with('!') {
        return None;
    }
    let words: Vec<&str> =
        text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')).collect();
    let used: Vec<&str> =
        vars.iter().map(String::as_str).filter(|var| words.contains(var)).collect();
    let generics = if used.is_empty() { String::new() } else { format!("[{}] ", used.join(", ")) };
    let src = format!("class X\n  def m: {generics}() -> {text}\nend\n");
    let sigs = crate::rbs::parse_signatures(&src).ok()?;
    let (_, sig) = sigs.methods.into_iter().next()?;
    match sig {
        Ty::Fn { ret, .. } => Some(std::sync::Arc::unwrap_or_clone(ret)),
        _ => None,
    }
}

/// The type variables in scope at byte `at` of `file`: those a
/// `#: [U] (...) -> ...` signature declares for its method, and those a
/// `#: [K, V]` line above a class or module declares for its body.
fn type_vars_at(file: &str, at: usize) -> Vec<String> {
    let Some(scopes) = sources::type_var_scopes(file, generic_scopes) else {
        return Vec::new();
    };
    let mut vars: Vec<String> = Vec::new();
    for (start, end, names) in scopes.iter() {
        if (*start..*end).contains(&at) {
            for name in names {
                if !vars.contains(name) {
                    vars.push(name.clone());
                }
            }
        }
    }
    vars
}

/// Every def, class and module in `text` whose `#:` comment opens with
/// a type parameter list, with its byte range.
fn generic_scopes(text: &str) -> Vec<sources::TypeVarScope> {
    if !text.contains("#: [") {
        return Vec::new();
    }
    struct Scopes<'a> {
        source: &'a [u8],
        out: Vec<sources::TypeVarScope>,
    }
    impl Scopes<'_> {
        fn record(&mut self, location: &ruby_prism::Location<'_>) {
            let start = location.start_offset();
            if let Some(names) =
                super::sorbet_sig::rbs_comment_text(self.source, start).and_then(|sig| type_param_names(&sig))
            {
                self.out.push((start, location.end_offset(), names));
            }
        }
    }
    impl<'pr> ruby_prism::Visit<'pr> for Scopes<'_> {
        fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
            self.record(&node.location());
            ruby_prism::visit_def_node(self, node);
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            self.record(&node.location());
            ruby_prism::visit_class_node(self, node);
        }
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            self.record(&node.location());
            ruby_prism::visit_module_node(self, node);
        }
    }
    let result = ruby_prism::parse(text.as_bytes());
    let mut scopes = Scopes { source: text.as_bytes(), out: Vec::new() };
    ruby_prism::Visit::visit(&mut scopes, &result.node());
    scopes.out
}

/// The names in a leading RBS type parameter list, `[out K, unchecked
/// in V < Comparable, E = String]` -> `K, V, E`.
fn type_param_names(sig: &str) -> Option<Vec<String>> {
    let rest = sig.strip_prefix('[')?;
    let mut depth = 0usize;
    let mut close = None;
    for (i, c) in rest.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            ']' => {
                close = Some(i);
                break;
            }
            _ => {}
        }
    }
    let list = &rest[..close?];
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut from = 0;
    for (i, c) in list.char_indices().chain(std::iter::once((list.len(), ','))) {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let name = list[from..i]
                    .split_whitespace()
                    .find(|w| !matches!(*w, "unchecked" | "out" | "in"))?;
                if !name.starts_with(|c: char| c.is_ascii_uppercase())
                    || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    return None;
                }
                names.push(name.to_string());
                from = i + 1;
            }
            _ => {}
        }
    }
    Some(names)
}
