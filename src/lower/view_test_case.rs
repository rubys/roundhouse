//! `ActionView::TestCase`'s `view` — the object Rails hands a helper
//! test — resolved to the helper module that defines the method.
//!
//! Rails builds `view` as an `ActionView::Base` with EVERY helper
//! module mixed in, so `view.message_presentation(message)` is an
//! ordinary send into whichever module defined that name. A view is a
//! per-request object a compiled target has no lane for, and mixing N
//! modules into one class is the dynamic shape this pipeline already
//! answers elsewhere by resolving the name at compile time: a helper
//! lowers to a module function (`MessagesHelper.message_presentation`),
//! and a bare call in a template is bound through
//! [`crate::app::App::helper_method_index`] — the flat union Rails'
//! include order produces, last-writer-wins.
//!
//! So `view.<m>(args)` is bound through that same index:
//!
//! ```ruby
//! view.message_presentation(message)  # → MessagesHelper.message_presentation(message)
//! ```
//!
//! THE INDEX IS THE RULE, not the test class's name. `MessagesHelperTest`
//! does name its subject the way `ActionCable::Channel::TestCase` does,
//! and Rails uses that name for one thing only — deciding which module
//! to `include` FIRST. Every other helper is mixed in too, so a test
//! that reaches for a neighbour's method gets it, and resolving by name
//! alone would bind the wrong module (or nothing) the first time one
//! does. Rails' answer is the union; this is the union.
//!
//! WHAT IT DOES NOT REWRITE: a `view` call whose name no helper module
//! defines. That is `render`, `assigns`, `output_buffer` and the rest of
//! `ActionView::Base`'s own surface — a real gap rather than a
//! resolution failure, so it is reported and left as written, and the
//! test fails naming the method it wanted.

use crate::app::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use std::collections::HashMap;

const VIEW_TEST_CASE: &str = "ActionView::TestCase";

pub fn apply_view_test_case_lowering(app: &mut App) -> Vec<Diagnostic> {
    let index: HashMap<Symbol, ClassId> = app.helper_method_index.clone();
    let mut diags = Vec::new();
    for tm in &mut app.test_modules {
        if !tm.parent.as_ref().is_some_and(|p| p.0.as_str() == VIEW_TEST_CASE) {
            continue;
        }
        let mut rewrite = |e: &mut Expr| rewrite(e, &index, &mut diags);
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
    diags
}

fn rewrite(e: &mut Expr, index: &HashMap<Symbol, ClassId>, diags: &mut Vec<Diagnostic>) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, index, diags));
    let span = e.span;
    let ExprNode::Send { recv, method, .. } = &mut *e.node else { return };
    if !recv.as_ref().is_some_and(is_view) {
        return;
    }
    let Some(module) = index.get(method) else {
        let mut d = Diagnostic::unsupported(
            span,
            None,
            "view",
            format!(
                "`view.{}` in an ActionView::TestCase is served only for a method one of the app's own `app/helpers/` modules defines; ActionView::Base's own surface (render, assigns, …) is not modeled",
                method.as_str()
            ),
        );
        d.severity = Severity::Warning;
        diags.push(d);
        return;
    };
    *recv = Some(Expr::new(
        span,
        ExprNode::Const {
            path: module.0.as_str().split("::").map(Symbol::from).collect(),
        },
    ));
}

/// The bare receiver `view` — a local-looking read with no receiver of
/// its own. A test that bound a local named `view` would be shadowing
/// the harness's method, which no corpus test does and which Rails
/// would resolve the same way.
fn is_view(recv: &Expr) -> bool {
    match &*recv.node {
        ExprNode::Send { recv: None, method, args, block: None, .. } => {
            method.as_str() == "view" && args.is_empty()
        }
        ExprNode::Var { name, .. } => name.as_str() == "view",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::TestModule;

    fn test_module(parent: &str, body: &str) -> TestModule {
        let src = format!("class MessagesHelperTest < {parent}\n  test \"t\" do\n    {body}\n  end\nend\n");
        let mut mods =
            crate::ingest::test::ingest_test_files(src.as_bytes(), "t.rb").expect("ingest");
        mods.remove(0)
    }

    fn app_with(tm: TestModule, helpers: &[(&str, &str)]) -> App {
        let mut app = App::new();
        for (method, module) in helpers {
            app.helper_method_index
                .insert(Symbol::from(*method), ClassId(Symbol::from(*module)));
        }
        app.test_modules.push(tm);
        app
    }

    fn lowered(app: &App) -> String {
        crate::emit::ruby::emit_expr(&app.test_modules[0].tests[0].body)
    }

    /// The index binds the module, including for a method the test
    /// class's own name does not name — Rails mixes every helper in.
    #[test]
    fn a_view_call_binds_the_module_that_defines_it() {
        let tm = test_module(
            VIEW_TEST_CASE,
            "a = view.message_presentation(message)\n    b = view.local_datetime_tag(t)",
        );
        let mut app = app_with(
            tm,
            &[
                ("message_presentation", "MessagesHelper"),
                ("local_datetime_tag", "TimeHelper"),
            ],
        );
        let diags = apply_view_test_case_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        let out = lowered(&app);
        assert!(out.contains("MessagesHelper.message_presentation(message)"), "{out}");
        assert!(out.contains("TimeHelper.local_datetime_tag(t)"), "{out}");
    }

    /// `ActionView::Base`'s own surface is a gap, reported and left.
    #[test]
    fn a_view_call_no_helper_defines_is_reported_and_left() {
        let tm = test_module(VIEW_TEST_CASE, "view.render(partial: \"x\")");
        let mut app = app_with(tm, &[("message_presentation", "MessagesHelper")]);
        let diags = apply_view_test_case_lowering(&mut app);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(lowered(&app).contains("view.render"), "{}", lowered(&app));
    }

    /// Only under the view parent: a model test with a local named
    /// `view` is not this construct.
    #[test]
    fn another_parents_test_is_untouched() {
        let tm = test_module("ActiveSupport::TestCase", "view.message_presentation(message)");
        let mut app = app_with(tm, &[("message_presentation", "MessagesHelper")]);
        let diags = apply_view_test_case_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        assert!(lowered(&app).contains("view.message_presentation"), "{}", lowered(&app));
    }
}
