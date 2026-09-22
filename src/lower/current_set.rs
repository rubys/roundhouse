//! `Current.set(request: r) { … }` → save, assign, run, restore.
//!
//! `ActiveSupport::CurrentAttributes.set` is a scoped write: it assigns
//! the named attributes, yields, and puts the previous values back —
//! including when the block raises, which is what keeps one request's
//! `Current.user` out of the next. Rails implements it by walking the
//! attribute names at run time; there is no list to walk here, because
//! `ingest::current_attributes` flattens a `Current` into ordinary
//! per-attribute accessors on a thread-local instance.
//!
//! The call site names the attributes, so the triple is spelled out at
//! lower time:
//!
//! ```ruby
//! Current.set(request: r) { body }
//! # →
//! __current_set_request = Current.request
//! Current.request = r
//! begin
//!   body
//! ensure
//!   Current.request = __current_set_request
//! end
//! ```
//!
//! ENSURE, NOT A TRAILING ASSIGNMENT, and it is the half that matters:
//! `Current.set` exists to be exception-safe. campfire's
//! `actiontext_opengraph_embeds_test` asserts inside the block, so the
//! first failing assertion would otherwise leave `Current.request`
//! pointing at a test host for every test after it — a leak that shows
//! up as an unrelated file failing, which is the worst kind.
//!
//! NO RUNTIME METHOD, deliberately. A generated `Current.set` would
//! need one signature covering every subset of attributes a caller
//! might name, and optional keywords are their own hazard on the strict
//! targets (docs/pipeline/runtime.md). The shape above uses only the
//! accessors the flattening already emits, so it compiles wherever they
//! do.
//!
//! WHAT IT DECLINES: a `set` whose argument is not a literal keyword
//! hash, or whose receiver is not one of the app's own
//! `CurrentAttributes` classes. Both are reported and left as written —
//! the tree has no `set` to call, so the site fails loudly with the
//! reason rather than running the block with the wrong scope.

use crate::app::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use std::collections::BTreeSet;

pub fn apply_current_set_lowering(app: &mut App) -> Vec<Diagnostic> {
    let classes: BTreeSet<String> = app
        .current_attribute_classes
        .iter()
        .map(|c| c.0.as_str().to_string())
        .collect();
    if classes.is_empty() {
        return Vec::new();
    }
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |e| rewrite(e, &classes, &mut diags));
    for view in &mut app.views {
        rewrite(&mut view.body, &classes, &mut diags);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &classes, &mut diags);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &classes, &mut diags);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &classes, &mut diags);
        }
    }
    diags
}

fn rewrite(e: &mut Expr, classes: &BTreeSet<String>, diags: &mut Vec<Diagnostic>) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, classes, diags));
    let span = e.span;
    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*e.node else { return };
    if method.as_str() != "set" {
        return;
    }
    let ExprNode::Const { path } = &*recv.node else { return };
    let class = path.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::");
    if !classes.contains(&class) {
        return;
    }
    let Some(body) = block else {
        report(span, &class, "it takes no block, so there is no scope to restore after", diags);
        return;
    };
    let Some(pairs) = keyword_pairs(args) else {
        report(span, &class, "its argument is not a literal keyword hash", diags);
        return;
    };
    *e = expand(span, &class, &pairs, body);
}

/// `[(attribute, value)]` from the single trailing keyword hash.
fn keyword_pairs(args: &[Expr]) -> Option<Vec<(String, Expr)>> {
    let [opts] = args else { return None };
    let ExprNode::Hash { entries, .. } = &*opts.node else { return None };
    if entries.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (k, v) in entries {
        let name = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => return None,
        };
        out.push((name, v.clone()));
    }
    Some(out)
}

fn expand(span: Span, class: &str, pairs: &[(String, Expr)], body: &Expr) -> Expr {
    let mut stmts: Vec<Expr> = Vec::new();
    // Saves FIRST, all of them, then the assignments — the order Rails'
    // own `set` uses. A save that read an attribute another pair had
    // already overwritten would restore the wrong value on a call that
    // names two.
    let saved: Vec<(String, Symbol)> = pairs
        .iter()
        .map(|(name, _)| (name.clone(), Symbol::from(format!("__current_set_{name}"))))
        .collect();
    for ((name, _), (_, local)) in pairs.iter().zip(saved.iter()) {
        stmts.push(assign_local(span, local.clone(), class_read(span, class, name)));
    }
    for (name, value) in pairs {
        stmts.push(class_write(span, class, name, value.clone()));
    }
    let mut restore: Vec<Expr> = Vec::new();
    for (name, local) in &saved {
        restore.push(class_write(span, class, name, local_read(span, local.clone())));
    }
    stmts.push(Expr::new(
        span,
        ExprNode::BeginRescue {
            body: body.clone(),
            rescues: Vec::new(),
            else_branch: None,
            ensure: Some(seq(span, restore)),
            implicit: false,
        },
    ));
    seq(span, stmts)
}

fn seq(span: Span, exprs: Vec<Expr>) -> Expr {
    if exprs.len() == 1 {
        return exprs.into_iter().next().expect("checked");
    }
    Expr::new(span, ExprNode::Seq { exprs })
}

fn class_read(span: Span, class: &str, name: &str) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(span, class)),
            method: Symbol::from(name),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

fn class_write(span: Span, class: &str, name: &str, value: Expr) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(span, class)),
            method: Symbol::from(format!("{name}=")),
            args: vec![value],
            block: None,
            parenthesized: false,
        },
    )
}

fn assign_local(span: Span, name: Symbol, value: Expr) -> Expr {
    Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name },
            value,
        },
    )
}

fn local_read(span: Span, name: Symbol) -> Expr {
    Expr::new(span, ExprNode::Var { id: VarId(0), name })
}

fn const_path(span: Span, class: &str) -> Expr {
    Expr::new(span, ExprNode::Const { path: class.split("::").map(Symbol::from).collect() })
}

fn report(span: Span, class: &str, why: &str, diags: &mut Vec<Diagnostic>) {
    let mut d = Diagnostic::unsupported(
        span,
        None,
        "CurrentAttributes#set",
        format!(
            "`{class}.set` is served only as a block form over a literal keyword hash, which is what lets the save/restore be spelled at compile time: {why}"
        ),
    );
    d.severity = Severity::Warning;
    diags.push(d);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::ClassId;

    fn app_with(body: &str) -> App {
        let src = format!("class T < ActiveSupport::TestCase\n  test \"t\" do\n    {body}\n  end\nend\n");
        let mut app = App::new();
        app.current_attribute_classes.push(ClassId(Symbol::from("Current")));
        app.test_modules
            .extend(crate::ingest::test::ingest_test_files(src.as_bytes(), "t.rb").expect("ingest"));
        app
    }

    fn lowered(app: &App) -> String {
        crate::emit::ruby::emit_expr(&app.test_modules[0].tests[0].body)
    }

    #[test]
    fn one_attribute_saves_assigns_and_restores_in_an_ensure() {
        let mut app = app_with("Current.set(request: r) do\n      work\n    end");
        let diags = apply_current_set_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        let out = lowered(&app);
        assert!(out.contains("__current_set_request = Current.request"), "{out}");
        assert!(out.contains("Current.request = r"), "{out}");
        assert!(out.contains("ensure"), "{out}");
        assert!(out.contains("Current.request = __current_set_request"), "{out}");
    }

    /// Every save runs before any assignment: with two attributes, a
    /// save that read after a write would restore the new value.
    #[test]
    fn two_attributes_save_before_either_is_written() {
        let mut app = app_with("Current.set(request: r, user: u) do\n      work\n    end");
        let diags = apply_current_set_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        let out = lowered(&app);
        let save_user = out.find("__current_set_user = Current.user").expect(&out);
        let write_request = out.find("Current.request = r").expect(&out);
        assert!(save_user < write_request, "a save ran after a write:\n{out}");
    }

    #[test]
    fn a_non_keyword_argument_is_reported_and_left() {
        let mut app = app_with("Current.set(attrs) do\n      work\n    end");
        let diags = apply_current_set_lowering(&mut app);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(lowered(&app).contains("Current.set(attrs)"), "{}", lowered(&app));
    }

    /// `set` on anything that is not one of the app's own
    /// CurrentAttributes classes is somebody else's method.
    #[test]
    fn another_receivers_set_is_untouched() {
        let mut app = app_with("Cache.set(key: k) do\n      work\n    end");
        let diags = apply_current_set_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        assert!(lowered(&app).contains("Cache.set"), "{}", lowered(&app));
    }
}
