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
//! `view.controller` is the one Base surface campfire's helper tests
//! need: they stub `perform_caching` on it so presentation caching
//! can be asserted. The harness already installs an
//! `ActionController::Base` as `ActionController::Current.controller`
//! (see `runtime/*/test/test_helper.rb`), and helpers lower their own
//! `controller` reads through that same slot — so `view.controller`
//! rewrites to `ActionController::Current.controller` and a mocha stub
//! lands on the object the helper will read.
//!
//! WHAT IT DOES NOT REWRITE: any other `view` call whose name no helper
//! module defines (`render`, `assigns`, `output_buffer`, …). Those stay
//! reported gaps.

use crate::app::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use std::collections::HashMap;

const VIEW_TEST_CASE: &str = "ActionView::TestCase";

pub fn apply_view_test_case_lowering(app: &mut App) -> Vec<Diagnostic> {
    let index: HashMap<Symbol, ClassId> = app.helper_method_index.clone();
    // Each app module's own methods, for the bare-call half below.
    let module_methods: HashMap<ClassId, std::collections::HashSet<Symbol>> = app
        .library_classes
        .iter()
        .filter(|lc| lc.is_module)
        .map(|lc| (lc.name.clone(), lc.methods.iter().map(|m| m.name.clone()).collect()))
        .collect();
    let mut diags = Vec::new();
    for tm in &mut app.test_modules {
        if !tm.parent.as_ref().is_some_and(|p| p.0.as_str() == VIEW_TEST_CASE) {
            continue;
        }
        // BARE calls resolve on the test instance, which is not `view`:
        // Rails mixes into the test class only the helper its name
        // names (`RichTextHelperTest` -> `RichTextHelper`) and the
        // modules the class body `include`s (`ContentFiltersTest`'s
        // `include MessagesHelper`). Later includes win, as Ruby's
        // ancestor order has it; the test's own methods win over all.
        let mut bare_modules: Vec<ClassId> = tm.includes.iter().rev().cloned().collect();
        bare_modules.extend(tm.target.iter().cloned());
        let own: std::collections::HashSet<Symbol> =
            tm.helpers.iter().map(|m| m.name.clone()).collect();
        let bare: HashMap<Symbol, ClassId> = bare_modules
            .iter()
            .rev()
            .filter_map(|m| module_methods.get(m).map(|ms| (m, ms)))
            .flat_map(|(m, ms)| ms.iter().map(move |name| (name.clone(), m.clone())))
            .filter(|(name, _)| !own.contains(name))
            .collect();
        let mut rewrite = |e: &mut Expr| {
            rewrite(e, &index, &mut diags);
            rewrite_bare(e, &bare);
        };
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
    // `view.controller` → the harness's current controller (see module
    // docs). Must run before the helper-index miss path, or the site
    // stays a warning and the mocha stub never reaches the helper.
    if method.as_str() == "controller" {
        *e = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Const {
                        path: vec![
                            Symbol::from("ActionController"),
                            Symbol::from("Current"),
                        ],
                    },
                )),
                method: Symbol::from("controller"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
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

/// `editable_body(message)` -> `RichTextHelper.editable_body(message)`:
/// a receiverless call the test class's mixed-in helpers define, bound
/// to the module the way a `view.` call is. (`bare` is built with the
/// winning module last, so the collect's last-writer-wins is Ruby's.)
fn rewrite_bare(e: &mut Expr, bare: &HashMap<Symbol, ClassId>) {
    e.node.for_each_child_mut(&mut |c| rewrite_bare(c, bare));
    let span = e.span;
    let ExprNode::Send { recv, method, .. } = &mut *e.node else { return };
    if recv.is_some() {
        return;
    }
    let Some(module) = bare.get(method) else { return };
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

    /// A BARE call resolves on the test instance: the helper the class
    /// is named for, and what it `include`s — Rails' mixins for an
    /// `ActionView::TestCase`, not every helper as `view` has.
    #[test]
    fn a_bare_call_binds_the_namesake_or_an_included_helper() {
        let src = "class RichTextHelperTest < ActionView::TestCase\n  include MessagesHelper\n  test \"t\" do\n    a = editable_body(m)\n    b = message_presentation(m)\n    c = unrelated(m)\n  end\nend\n";
        let mut mods = crate::ingest::test::ingest_test_files(src.as_bytes(), "t.rb").expect("ingest");
        let mut app = App::new();
        app.test_modules.push(mods.remove(0));
        for module in [
            "module RichTextHelper\n  def editable_body(m)\n    m\n  end\nend\n",
            "module MessagesHelper\n  def message_presentation(m)\n    m\n  end\nend\n",
            "module OtherHelper\n  def unrelated(m)\n    m\n  end\nend\n",
        ] {
            app.library_classes.extend(
                crate::ingest::ingest_library_classes(module.as_bytes(), "h.rb").expect("helper"),
            );
        }
        apply_view_test_case_lowering(&mut app);
        let out = lowered(&app);
        assert!(out.contains("RichTextHelper.editable_body(m)"), "{out}");
        assert!(out.contains("MessagesHelper.message_presentation(m)"), "{out}");
        assert!(out.contains("c = unrelated(m)"), "not mixed in, so not bound:\n{out}");
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

    /// campfire stubs `perform_caching` on `view.controller` — that is
    /// the harness's `ActionController::Current.controller`.
    #[test]
    fn view_controller_is_the_harness_current_controller() {
        let tm = test_module(
            VIEW_TEST_CASE,
            "view.controller.stubs(:perform_caching).returns(true)",
        );
        let mut app = app_with(tm, &[("message_presentation", "MessagesHelper")]);
        let diags = apply_view_test_case_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        let out = lowered(&app);
        assert!(
            out.contains("ActionController::Current.controller.stubs"),
            "{out}"
        );
        assert!(!out.contains("view.controller"), "{out}");
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
