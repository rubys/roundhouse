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
//! value = r
//! current = Current.instance
//! saved = false
//! begin
//!   old = current.request
//!   saved = true
//!   current.request = value
//!   current.then { body }
//! ensure
//!   current.request = old if saved
//! end
//! ```
//!
//! All RHSs run before selecting the instance. Rails' Object#with then
//! saves and writes each slot sequentially: a writer can change the next
//! getter's answer. Setup belongs INSIDE ensure; a failing getter has not
//! saved its slot, whereas a failing writer has. Restoration runs in the
//! original order and stops if a restore writer raises. Keep the user's
//! attached block (not a lambda value or `.call`) so result, receiver,
//! break/next and nonlocal return survive.
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
//! targets (docs/pipeline/runtime.md). The shape above uses the flattened
//! accessors and Kernel#then for the attached block. A target must support
//! that block invocation as well as the scoped ensure.
//!
//! WHAT IT DECLINES: a `set` whose argument is not a literal keyword
//! hash, or whose receiver is not one of the app's own
//! `CurrentAttributes` classes. Both are reported and left as written —
//! the tree has no `set` to call, so the site fails loudly with the
//! reason rather than running the block with the wrong scope.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{LibraryClass, MethodReceiver};
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
    // Replacing Rails #set/#with and calling `.then` is equivalent only
    // when app methods/accessors/includes do not intercept those sends.
    let global_shadow = app.library_classes.iter()
        .filter(|c| matches!(c.name.0.as_str(), "Object" | "Kernel"))
        .any(|c| scope_dispatch_unproven(c, false));
    let shadowed: BTreeSet<String> = app.library_classes.iter()
        .filter(|c| classes.contains(c.name.0.as_str()))
        .filter(|c| global_shadow || scope_dispatch_unproven(c, true))
        .map(|c| c.name.0.as_str().to_owned()).collect();
    // Reserve source identifiers too: an unused formal or pattern binding
    // need not appear as a Var in the body. The conservative token inventory
    // also covers block-local names without depending on their reads.
    let mut used: BTreeSet<String> = app.sources.iter().flat_map(|s| {
        s.text.split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty()).map(str::to_owned)
    }).collect();
    super::for_each_hook_body_ref(app, &mut |e| collect_names(e, &mut used));
    for view in &app.views { collect_names(&view.body, &mut used); }
    for tm in &app.test_modules {
        if let Some(setup) = &tm.setup { collect_names(setup, &mut used); }
        for t in &tm.tests { collect_names(&t.body, &mut used); }
        for m in &tm.helpers { collect_names(&m.body, &mut used); }
    }
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |e| rewrite(e, &classes, &shadowed, &mut used, &mut diags));
    for view in &mut app.views {
        rewrite(&mut view.body, &classes, &shadowed, &mut used, &mut diags);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &classes, &shadowed, &mut used, &mut diags);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &classes, &shadowed, &mut used, &mut diags);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &classes, &shadowed, &mut used, &mut diags);
        }
    }
    diags
}

fn scope_dispatch_unproven(class: &LibraryClass, class_set: bool) -> bool {
    // Library IR omits some effect-only module reopens, including across
    // files. An apparently harmless first fragment cannot prove the full
    // included dispatch graph. Keep that boundary until it is source-closed.
    !class.includes.is_empty() || class.methods.iter().any(|m|
        (m.receiver == MethodReceiver::Instance && matches!(m.name.as_str(), "set" | "with" | "then"))
            || (class_set && m.receiver == MethodReceiver::Class && m.name.as_str() == "set"))
}

fn rewrite(e: &mut Expr, classes: &BTreeSet<String>, shadowed: &BTreeSet<String>, used: &mut BTreeSet<String>, diags: &mut Vec<Diagnostic>) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, classes, shadowed, used, diags));
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
    if shadowed.contains(&class) {
        report(span, &class, "app-owned `set`, `with`, `then` dispatch or unresolved included ancestry cannot prove Rails' scoped direct-yield semantics", diags);
        return;
    }
    let Some(body) = block else {
        report(span, &class, "it takes no block, so there is no scope to restore after", diags);
        return;
    };
    let Some(pairs) = keyword_pairs(args) else {
        report(span, &class, "its argument is not a nonempty literal hash with distinct attribute names", diags);
        return;
    };
    let mut replacement = expand(span, &class, &pairs, body, used);
    replacement.ty = e.ty.clone();
    replacement.decisions = e.decisions | crate::expr::CURRENT_SET_SCOPE;
    *e = replacement;
}

/// `[(attribute, value)]` from the single trailing keyword hash.
fn keyword_pairs(args: &[Expr]) -> Option<Vec<(String, Expr)>> {
    let [opts] = args else { return None };
    let ExprNode::Hash { entries, .. } = &*opts.node else { return None };
    if entries.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for (k, v) in entries {
        let name = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => return None,
        };
        // Ruby Hash overwrites duplicate keys before #with sees them.
        // Do not turn that into multiple saves/writes/restores.
        if !names.insert(name.clone()) { return None; }
        out.push((name, v.clone()));
    }
    Some(out)
}

fn expand(span: Span, class: &str, pairs: &[(String, Expr)], body: &Expr, used: &mut BTreeSet<String>) -> Expr {
    let mut stmts: Vec<Expr> = Vec::new();
    let mut values = Vec::new();
    for (_, value) in pairs {
        let local = fresh("value", used);
        stmts.push(assign_local(span, local.clone(), value.clone()));
        values.push(local);
    }
    let current = fresh("instance", used);
    stmts.push(assign_local(span, current.clone(), send(span, const_path(span, class), "instance", vec![])));
    let mut setup = Vec::new();
    let mut restore = Vec::new();
    for ((name, _), value) in pairs.iter().zip(values) {
        let old = fresh("old", used);
        let saved = fresh("saved", used);
        stmts.push(assign_local(span, saved.clone(), boolean(span, false)));
        setup.push(assign_local(span, old.clone(), send(span, local_read(span, current.clone()), name, vec![])));
        setup.push(assign_local(span, saved.clone(), boolean(span, true)));
        setup.push(send(span, local_read(span, current.clone()), &format!("{name}="), vec![local_read(span, value)]));
        restore.push(Expr::new(span, ExprNode::If {
            cond: local_read(span, saved),
            then_branch: send(span, local_read(span, current.clone()), &format!("{name}="), vec![local_read(span, old)]),
            else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        }));
    }
    setup.push(Expr::new(span, ExprNode::Send {
        recv: Some(local_read(span, current)),
        method: Symbol::from("then"),
        args: vec![],
        block: Some(body.clone()),
        parenthesized: false,
    }));
    stmts.push(Expr::new(
        span,
        ExprNode::BeginRescue {
            body: seq(span, setup),
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

fn send(span: Span, receiver: Expr, name: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(receiver),
            method: Symbol::from(name),
            args,
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

fn boolean(span: Span, value: bool) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Bool { value } })
}

fn fresh(role: &str, used: &mut BTreeSet<String>) -> Symbol {
    let base = format!("__rh_current_{role}");
    let mut name = base.clone();
    let mut suffix = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}_{suffix}");
        suffix += 1;
    }
    Symbol::from(name)
}

fn collect_names(expr: &Expr, used: &mut BTreeSet<String>) {
    match &*expr.node {
        ExprNode::Var { name, .. } | ExprNode::Let { name, .. }
        | ExprNode::Assign { target: LValue::Var { name, .. }, .. }
        | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } => { used.insert(name.as_str().into()); }
        ExprNode::Lambda { params, rest_param, extra_params, block_param, .. } => {
            used.extend(params.iter().chain(rest_param.iter()).chain(block_param.iter()).map(|n| n.as_str().to_owned()));
            used.extend(extra_params.iter().map(|p| p.name.as_str().to_owned()));
        }
        ExprNode::BeginRescue { rescues, .. } => {
            used.extend(rescues.iter().filter_map(|r| r.binding.as_ref()).map(|n| n.as_str().to_owned()));
        }
        _ => {}
    }
    expr.node.for_each_child(&mut |c| collect_names(c, used));
}

fn report(span: Span, class: &str, why: &str, diags: &mut Vec<Diagnostic>) {
    diags.push(Diagnostic::unsupported(
        span,
        None,
        "CurrentAttributes#set",
        format!(
            "`{class}.set` is served only as a block form over a literal keyword hash, which is what lets the save/restore be spelled at compile time: {why}"
        ),
    ));
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
