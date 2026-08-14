//! Time-vocabulary grounding: the Rails-isms on a `Time` that no
//! target's `Time` answers — `Time.current`, `httpdate`, and `to_fs`.
//!
//! Plain Ruby has no `Time.current` — the Rails-ism is as undefined on
//! the CRuby tree as under spinel AOT, just lazily so — and the corpus
//! apps run UTC, where the two are second-for-second equivalent.
//! Grounding it here keeps `Time` un-reopened (built-in reopening in
//! the shared runtime is off-limits) and lands on vocabulary every
//! emitter already speaks: all target families handle `Time.now` and a
//! zero-arg `.utc`, while only the ruby family ever knew `current`.
//!
//! Runs post-analyze (with `apply_blank_lowering`, see
//! `apply_post_analyze_lowerings`): the rewrite is shape-directed, not
//! type-directed, but rewriting after the analyzer means `Time.current`
//! stays typeable as a registered class method and the new nodes can be
//! stamped from the types analyze already assigned. No residue policy —
//! the match (`Time.current`, zero args, no block) is unconditional and
//! the rewrite total, so there is no diagnostic to return.
//!
//! View bodies are deliberately not walked — same carve-out as the
//! blank pass: the view pipeline still applies the ruby-family emit
//! copy of this rewrite (`emit::ruby::library::apply_time_current_
//! lowering`) to lowered view classes, and rejoins the shared home when
//! views migrate. Test-module and fixture bodies are not walked either
//! (they run on CRuby; extendable when a strict-target test lane needs
//! it).
//!
//! ## `to_fs(:format)`
//!
//! `to_fs` is a hole we opened ourselves: `analyze::body::send`
//! TYPES it (Str) and `routes_to_library::direct` EMITS
//! `updated_at.to_fs(:number)` into every `direct`-generated URL
//! helper, while no target implements it — so campfire's sign-in page
//! raised on `fresh_account_logo_path` and its message list raised
//! inside a `rescue Exception` that hid the cause.
//!
//! Rails resolves it through `Time::DATE_FORMATS`, a hash of strftime
//! strings and lambdas. Both halves are compile-time knowledge, so both
//! expand here rather than becoming a runtime method on nine `Time`s:
//! a built-in maps to the `strftime` every emitter already speaks, and
//! an APP-DEFINED format inlines the lambda body the initializer scan
//! recorded (`App::time_formats`).
//!
//! An unrecognized format DECLINES, loudly. Rails' own fallback is
//! `to_s`, and emitting that would be the worst outcome available: "we
//! do not know this format" is not the same fact as "Rails does not
//! know it", and the two disagree exactly when an app defines a format
//! somewhere the initializer scan did not look.

use std::collections::BTreeMap;

use crate::app::App;
use crate::dialect::MethodDef;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

/// App-defined `to_fs` formats, as the lowering consults them.
pub(crate) type TimeFormats = BTreeMap<Symbol, MethodDef>;

/// Rewrite `Time.current` sends across every app body the post-analyze
/// hook owns (models, library classes, controllers, seeds — not views).
pub fn apply_time_current_lowering(app: &mut App) {
    let formats = std::mem::take(&mut app.time_formats);
    super::for_each_hook_body(app, &mut |body| rewrite_time_current(body, &formats));
    app.time_formats = formats;
}

/// `Time.current` → `Time.now.utc`, in place, recursively. The original
/// `Time` const node moves into the new tree (keeping its stamped type),
/// the synthesized `now` send takes the site's own type (`Time.now` and
/// `Time.current` type identically), and the outer expr keeps its type.
/// Also the implementation behind the ruby emitter's view-pipeline copy.
pub(crate) fn rewrite_time_current(expr: &mut Expr, formats: &TimeFormats) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite_time_current(c, formats));
    let is_target = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "current"
                && args.is_empty()
                && matches!(&*r.node,
                    ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Time")
    );
    if is_target {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv: Some(time_const), .. } = node else { unreachable!() };
        let mut now = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(time_const),
                method: Symbol::from("now"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        now.ty = expr.ty.clone();
        *expr.node = ExprNode::Send {
            recv: Some(now),
            method: Symbol::from("utc"),
            args: vec![],
            block: None,
            parenthesized: false,
        };
        return;
    }
    // `t.httpdate` — stdlib-`time` sugar neither the CRuby tree
    // (without a `require "time"`) nor AOT targets know. Ground to
    // its definition: `t.getutc.strftime("%a, %d %b %Y %H:%M:%S
    // GMT")` — `getutc`, not `utc`, which mutates its receiver.
    // Shape-directed on the zero-arg name; `httpdate` is
    // Time-specific vocabulary.
    let is_httpdate = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(_), method, args, block: None, .. }
            if method.as_str() == "httpdate" && args.is_empty()
    );
    if is_httpdate {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv: Some(t), .. } = node else { unreachable!() };
        let getutc = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(t),
                method: Symbol::from("getutc"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let fmt = Expr::new(
            span,
            ExprNode::Lit {
                value: crate::expr::Literal::Str { value: "%a, %d %b %Y %H:%M:%S GMT".into() },
            },
        );
        *expr.node = ExprNode::Send {
            recv: Some(getutc),
            method: Symbol::from("strftime"),
            args: vec![fmt],
            block: None,
            parenthesized: true,
        };
        return;
    }
    rewrite_to_fs(expr, formats);
}

/// Rails' built-in `Time::DATE_FORMATS` entries that are plain strftime
/// strings, MEASURED against Rails 8.1 rather than transcribed from the
/// source (`Time.utc(2026,8,14,9,5,3).to_fs(:<name>)`):
///
/// ```text
/// db      2026-08-14 09:05:03      short   14 Aug 09:05
/// number  20260814090503           long    August 14, 2026 09:05
/// time    09:05
/// ```
///
/// The rest of Rails' table is deliberately absent, each for a reason:
/// `:usec` / `:nsec` need `%6N` / `%9N`, which our per-target strftime
/// mappings do not all carry; `:rfc822` / `:long_ordinal` / `:inspect`
/// are lambdas whose output wants its own oracle run. They decline (and
/// say so) rather than shipping a format nobody measured.
fn builtin_time_format(name: &str) -> Option<&'static str> {
    Some(match name {
        "db" => "%Y-%m-%d %H:%M:%S",
        "number" => "%Y%m%d%H%M%S",
        "time" => "%H:%M",
        "short" => "%d %b %H:%M",
        "long" => "%B %d, %Y %H:%M",
        _ => return None,
    })
}

/// `<time>.to_fs(:format)` / `to_formatted_s(:format)` → what the format
/// is defined as. See the header for why an unknown one declines.
fn rewrite_to_fs(expr: &mut Expr, formats: &TimeFormats) {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if !matches!(method.as_str(), "to_fs" | "to_formatted_s") {
        return;
    }
    // Bare `to_fs` IS `to_s` — Rails' default format. Left alone: every
    // target answers `to_s`, and the no-arg call is not what breaks.
    let [format] = args.as_slice() else { return };
    let Some(name) = format_name(format) else {
        return decline(expr, "the format is not a literal");
    };
    let recv = recv.clone();
    let span = expr.span;

    // An app-defined format wins over a built-in, as it does in Rails —
    // the initializer assigns into the same hash.
    if let Some(method) = formats.get(&Symbol::from(name.as_str())) {
        let Some(param) = method.params.first() else {
            return decline(expr, "the app's format lambda takes no parameter");
        };
        // The receiver is substituted into the body once per mention of
        // the parameter, so a re-evaluated one would be a behavior
        // change, not just a slower one.
        if !crate::lower::case_lambda::is_pure_read(&recv) {
            return decline(expr, "the receiver is not a pure read to substitute");
        }
        let mut body = method.body.clone();
        crate::lower::case_lambda::subst(&mut body, &param.name, &recv);
        // The lambda's value is whatever it returns — campfire's `:epoch`
        // is an Integer — while `to_fs` is typed Str and every call site
        // renders it. `.to_s` reconciles the two, and matches what Rails
        // does with the value at those sites anyway.
        *expr.node = ExprNode::Send {
            recv: Some(body),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        };
        return;
    }

    let Some(fmt) = builtin_time_format(&name) else {
        return decline(expr, &format!("`{name}` is not a format we have measured"));
    };
    *expr.node = ExprNode::Send {
        recv: Some(recv),
        method: Symbol::from("strftime"),
        args: vec![Expr::new(
            span,
            ExprNode::Lit { value: crate::expr::Literal::Str { value: fmt.to_string() } },
        )],
        block: None,
        parenthesized: true,
    };
}

/// The format name a `to_fs` argument spells, if it is a literal.
fn format_name(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => {
            Some(value.as_str().to_string())
        }
        ExprNode::Lit { value: crate::expr::Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
}

fn decline(expr: &Expr, why: &str) {
    crate::emit::diagnostics::push(crate::lower::residue_diagnostic(
        "time_to_fs",
        "to_fs",
        expr.span,
        why,
        format!(
            "`to_fs` left in source shape ({why}) — no target implements it, \
             so this call will raise. Rails' own fallback is `to_s`, which is \
             deliberately NOT emitted here: it would render a readable date \
             where the app asked for a format, silently"
        ),
    ));
}
