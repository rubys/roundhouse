//! `"literal" << x` → `"literal" + x`.
//!
//! Appending to a string LITERAL is a Ruby idiom for building a long
//! string across source lines — lobsters' StoryRepository#top splits a
//! SQL fragment that way:
//!
//! ```ruby
//! where("created_at >= (DATETIME('now', '- " <<
//!   "#{length[:dur]} #{length[:intv].upcase}'))")
//! ```
//!
//! CRuby allows it because a literal without `# frozen_string_literal`
//! evaluates to a fresh mutable String each time. Spinel freezes string
//! literals, so the same source raises `FrozenError` at run time — it
//! took down `/top` on the transpiled app. Nothing observes the mutation
//! (the receiver is a temporary that only the `<<` result reaches), so
//! `+` is equivalent and frozen-safe.
//!
//! Two deliberate limits:
//!
//! - The receiver must be a string literal or interpolation. An lvalue
//!   receiver is the sibling case handled in `emit::ruby::library`
//!   (`<lv>.sub!` → `<lv> = <lv>.sub`), where reassignment is the right
//!   shape because the mutation IS observable through the name.
//! - The argument must be a String. `String#<<` with an Integer appends
//!   a CODEPOINT (`"a" << 65` is `"aA"`), which `+` cannot express — it
//!   would raise TypeError. Those sites keep the destructive call rather
//!   than get a silently wrong rewrite.
//!
//! A chain (`"a" << b << c`) parses as `("a" << b) << c`. Children are
//! rewritten first, so the inner becomes `"a" + b` and the outer's
//! receiver is then a Send rather than a literal — left alone, correctly:
//! it is mutating the fresh String `+` just produced, which is not frozen.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_literal_append_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

/// A string literal or interpolation — a fresh temporary in CRuby, a
/// frozen constant under spinel.
fn is_string_literal(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Lit { value: Literal::Str { .. } } | ExprNode::StringInterp { .. }
    )
}

/// Is this argument definitely a String? Anything else — an Integer
/// (codepoint append), or a type we can't see — is left alone.
fn is_string_arg(e: &Expr) -> bool {
    if matches!(e.ty, Some(Ty::Str)) {
        return true;
    }
    is_string_literal(e)
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "<<" || args.len() != 1 {
        return;
    }
    if !is_string_literal(recv) || !is_string_arg(&args[0]) {
        return;
    }
    if let ExprNode::Send { method, .. } = &mut *expr.node {
        *method = Symbol::from("+");
    }
}
