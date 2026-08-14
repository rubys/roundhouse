//! The turbo-rails Drive helper family (`Turbo::DriveHelper`):
//! `turbo_page_requires_reload`, `turbo_exempts_page_from_cache`,
//! `turbo_exempts_page_from_preview`, `turbo_refreshes_with`.
//!
//! Every one of them is defined as `provide :head, tag.meta(…)` — a
//! deposit into the `:head` slot the layout splices with `<%= yield
//! :head %>`, and NO template output of its own. That holds for the
//! `<%= %>` spelling too (campfire's rooms/show writes one): Rails'
//! `provide` returns nil when it is handed content, so the output
//! position renders "".
//!
//! Expanded at LOWER time into the deposit each helper is defined as,
//! rather than added to every target's view runtime. The markup is a
//! compile-time constant — there is nothing for a runtime to compute —
//! so inlining reaches all nine targets at once instead of nine
//! hand-written copies of the same string.
//!
//! The `_tag` variants (`turbo_page_requires_reload_tag`, …), which
//! return the markup WITHOUT depositing it, are deliberately absent:
//! no app in the corpus calls one, and each is one more row in the
//! table below when one does.

use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::span::Span;

use crate::lower::view::extract_sym_or_str;

use super::{lit_str, lit_sym, todo_io_append, view_helpers_call};

/// `tag.meta(name: …, content: …)` output, measured against
/// ActionView 8.2: a void element with no self-closing slash,
/// attributes in call order.
fn meta(name: &str, content: &str) -> String {
    format!("<meta name=\"{name}\" content=\"{content}\">")
}

/// `provide :head, <markup>`. The deposit APPENDS (see
/// `ViewHelpers.content_for_set`) — which is what lets a page carry
/// both a Drive directive and its own `content_for :head` block, as
/// campfire's rooms/show does.
fn provide_head(markup: String) -> Expr {
    view_helpers_call(
        "content_for_set",
        vec![lit_sym(Symbol::from("head")), lit_str(markup)],
    )
}

/// What a recognized Drive-helper call deposits into `:head`.
pub(crate) enum HeadDirective {
    /// One `<meta …>` per deposit, in call order.
    Metas(Vec<String>),
    /// A Drive helper we will not render, and the ledger tag saying
    /// why. turbo-rails raises ArgumentError on these.
    Declined(&'static str),
}

/// Recognize a Drive-helper call, or None when `method` names
/// something else. Shared with the Python view emitter, which walks
/// compiled ERB on its own and never sees this module's lowering — the
/// markup table is Rails knowledge and lives in one place, while each
/// walker renders the deposit its own way.
pub(crate) fn head_directive(method: &str, args: &[Expr]) -> Option<HeadDirective> {
    let markup = match method {
        "turbo_exempts_page_from_cache" => meta("turbo-cache-control", "no-cache"),
        "turbo_exempts_page_from_preview" => meta("turbo-cache-control", "no-preview"),
        "turbo_page_requires_reload" => meta("turbo-visit-control", "reload"),
        "turbo_refreshes_with" => return Some(refreshes_with(args)),
        _ => return None,
    };
    // The no-arg members take none; anything passed means we are
    // looking at a different method that happens to share the name.
    if !args.is_empty() {
        return None;
    }
    Some(HeadDirective::Metas(vec![markup]))
}

/// Recognize a Drive-helper call and return the statements it lowers
/// to, or None when `method` names something else. Callers use it from
/// both template positions — `<% turbo_page_requires_reload %>` and
/// `<%= turbo_exempts_page_from_preview %>` — because the answer is
/// the same either way: deposit, append nothing.
pub(super) fn emit_turbo_drive_directive(
    method: &str,
    args: &[Expr],
    span: Span,
) -> Option<Vec<Expr>> {
    match head_directive(method, args)? {
        HeadDirective::Metas(metas) => Some(metas.into_iter().map(provide_head).collect()),
        HeadDirective::Declined(why) => Some(vec![todo_io_append(why, span)]),
    }
}

/// `turbo_refreshes_with(method: :replace, scroll: :reset)` — two
/// deposits, one per directive. Both options are keyword-only with
/// defaults, and turbo-rails RAISES ArgumentError on any value outside
/// its two-element sets, so an unrecognized one is a template bug: it
/// files residue rather than rendering a directive Turbo would not
/// honor.
fn refreshes_with(args: &[Expr]) -> HeadDirective {
    let mut refresh_method = "replace";
    let mut scroll = "reset";

    if args.len() > 1 {
        return HeadDirective::Declined("turbo_refreshes_with arg shape");
    }
    if let Some(opts) = args.first() {
        let ExprNode::Hash { entries, .. } = &*opts.node else {
            return HeadDirective::Declined("turbo_refreshes_with non-literal options");
        };
        for (key, value) in entries {
            let (Some(key), Some(value)) = (extract_sym_or_str(key), extract_sym_or_str(value))
            else {
                return HeadDirective::Declined("turbo_refreshes_with non-literal options");
            };
            match key {
                "method" => refresh_method = value,
                "scroll" => scroll = value,
                _ => return HeadDirective::Declined("turbo_refreshes_with unknown option"),
            }
        }
    }
    if !matches!(refresh_method, "replace" | "morph") || !matches!(scroll, "reset" | "preserve") {
        return HeadDirective::Declined("turbo_refreshes_with option value outside turbo's set");
    }
    HeadDirective::Metas(vec![
        meta("turbo-refresh-method", refresh_method),
        meta("turbo-refresh-scroll", scroll),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Literal;

    fn sym(name: &str) -> Expr {
        lit_sym(Symbol::from(name))
    }

    /// The `content_for_set(:head, "<meta …>")` markup each call
    /// deposits, in order.
    fn deposits(method: &str, args: &[Expr]) -> Vec<String> {
        emit_turbo_drive_directive(method, args, Span::synthetic())
            .expect("recognized")
            .iter()
            .filter_map(|e| match &*e.node {
                ExprNode::Send { args, .. } => args.get(1).and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Str { value } } => Some(value.to_string()),
                    _ => None,
                }),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_no_arg_directives_deposit_their_meta_tag() {
        assert_eq!(
            deposits("turbo_exempts_page_from_preview", &[]),
            vec!["<meta name=\"turbo-cache-control\" content=\"no-preview\">"]
        );
        assert_eq!(
            deposits("turbo_exempts_page_from_cache", &[]),
            vec!["<meta name=\"turbo-cache-control\" content=\"no-cache\">"]
        );
        assert_eq!(
            deposits("turbo_page_requires_reload", &[]),
            vec!["<meta name=\"turbo-visit-control\" content=\"reload\">"]
        );
    }

    #[test]
    fn an_unrelated_helper_is_not_claimed() {
        assert!(emit_turbo_drive_directive("turbo_stream_from", &[], Span::synthetic()).is_none());
        // Same name, but the Drive members take no arguments — so this
        // is somebody else's method and the generic helper path keeps it.
        assert!(emit_turbo_drive_directive(
            "turbo_page_requires_reload",
            &[sym("x")],
            Span::synthetic()
        )
        .is_none());
    }

    #[test]
    fn refreshes_with_defaults_to_replace_and_reset() {
        assert_eq!(
            deposits("turbo_refreshes_with", &[]),
            vec![
                "<meta name=\"turbo-refresh-method\" content=\"replace\">",
                "<meta name=\"turbo-refresh-scroll\" content=\"reset\">",
            ]
        );
    }

    #[test]
    fn refreshes_with_honors_both_options() {
        let opts = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(sym("method"), sym("morph")), (sym("scroll"), sym("preserve"))],
                kwargs: true,
            },
        );
        assert_eq!(
            deposits("turbo_refreshes_with", &[opts]),
            vec![
                "<meta name=\"turbo-refresh-method\" content=\"morph\">",
                "<meta name=\"turbo-refresh-scroll\" content=\"preserve\">",
            ]
        );
    }

    /// turbo-rails raises ArgumentError here; we file residue and
    /// deposit nothing rather than emit a directive Turbo ignores.
    #[test]
    fn refreshes_with_declines_a_value_outside_turbos_set() {
        let opts = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(sym("method"), sym("teleport"))],
                kwargs: true,
            },
        );
        assert!(deposits("turbo_refreshes_with", &[opts]).is_empty());
    }
}
