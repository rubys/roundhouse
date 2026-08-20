//! `assert_enqueued_jobs 1, only: Bot::WebhookJob` → `only:
//! ["Bot::WebhookJob"]`.
//!
//! Rails' ActiveJob test helpers filter by job CLASS. A class is not a
//! first-class value on the strict targets — there is no object to pass
//! and nothing to compare — so the log those helpers read
//! (`ActiveJob::PERFORMED`, appended by the `perform_later` wrapper)
//! holds class NAMES, and the call site is rewritten to match.
//!
//! Compile time is where this belongs: the class is a literal at every
//! site, so nothing is lost by resolving it here, and the alternative
//! is a runtime class registry the strict targets cannot carry
//! ([[feedback_runtime_must_be_statically_resolvable]]).
//!
//! The filters (`only:`/`except:`) become an ARRAY in both spellings —
//! `only: X` and `only: [X, Y]` — so the helper's parameter is
//! `Array[String]` and monomorphic. An empty array is "any", which is
//! also what an absent `only:` means. `job:`, which names exactly one
//! class, becomes a plain String.
//!
//! Scoped to the helper NAMES and to TEST bodies. `only:` is a common
//! keyword; the narrowing is that the call has to be one of ActiveJob's
//! own assertions.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// The ActiveJob test assertions that take a job-class filter.
const JOB_FILTER_HELPERS: &[&str] = &[
    "assert_enqueued_jobs",
    "assert_no_enqueued_jobs",
    "perform_enqueued_jobs",
    "assert_performed_jobs",
    "assert_no_performed_jobs",
    "assert_enqueued_with",
];

pub fn apply_job_test_only_lowering(app: &mut App) {
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body);
        }
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else { return };
    if !JOB_FILTER_HELPERS.contains(&method.as_str()) {
        return;
    }
    for arg in args.iter_mut() {
        let ExprNode::Hash { entries, .. } = &mut *arg.node else { continue };
        for (key, value) in entries.iter_mut() {
            let ExprNode::Lit { value: Literal::Sym { value: k } } = &*key.node else { continue };
            // `job:` names ONE class (`assert_enqueued_with`), the
            // filters name a set.
            let single = k.as_str() == "job";
            if !single && k.as_str() != "only" && k.as_str() != "except" {
                continue;
            }
            let Some(names) = class_names(value) else { continue };
            let lit = |n: String, span| {
                Expr::new(span, ExprNode::Lit { value: Literal::Str { value: n } })
            };
            *value = if single {
                match names.into_iter().next() {
                    Some(n) => lit(n, value.span),
                    None => continue,
                }
            } else {
                Expr::new(
                    value.span,
                    ExprNode::Array {
                        elements: names.into_iter().map(|n| lit(n, value.span)).collect(),
                        style: Default::default(),
                    },
                )
            };
        }
    }
}

/// The class names an `only:` value denotes — one Const, or an array of
/// them. Anything else (a lambda, a local) is left alone: this pass
/// resolves literals, it does not evaluate.
fn class_names(value: &Expr) -> Option<Vec<String>> {
    let one = |e: &Expr| match &*e.node {
        ExprNode::Const { path } => Some(
            path.iter()
                .map(|s| s.as_str().trim_start_matches("::"))
                .collect::<Vec<_>>()
                .join("::"),
        ),
        _ => None,
    };
    match &*value.node {
        ExprNode::Const { .. } => one(value).map(|n| vec![n]),
        ExprNode::Array { elements, .. } => {
            elements.iter().map(one).collect::<Option<Vec<_>>>().filter(|v| !v.is_empty())
        }
        _ => None,
    }
}
