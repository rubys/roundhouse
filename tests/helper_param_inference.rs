//! View→helper param unification: a bare helper call in a template
//! (`shout("hi")`) is dispatch to the helper module Rails mixes into
//! every view, so its arg types are call-site evidence for the
//! helper's params. The fixpoint unifies them, and the post-fixpoint
//! stamp writes the discovered signature onto the helper `MethodDef`
//! so the emitted RBS carries it.

use roundhouse::analyze::Analyzer;
use roundhouse::dialect::View;
use roundhouse::expr::{Expr, ExprNode, Literal};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_library_classes;
use roundhouse::span::Span;
use roundhouse::ty::{Row, Ty};
use roundhouse::App;

fn helper_app(view_body: Expr) -> App {
    let mut app = App::new();
    let classes = ingest_library_classes(
        b"module ApplicationHelper\n  def shout(msg)\n    msg\n  end\nend\n",
        "app/helpers/application_helper.rb",
    )
    .expect("ingest helper");
    app.library_classes.extend(classes);
    app.helper_method_index
        .insert(Symbol::from("shout"), ClassId(Symbol::from("ApplicationHelper")));
    app.views.push(View {
        name: Symbol::from("posts/index"),
        format: Symbol::from("html"),
        locals: Row::default(),
        body: view_body,
        strict_locals: None,
    });
    app
}

fn bare_call(method: &str, arg: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(method),
            args: vec![arg],
            block: None,
            parenthesized: true,
        },
    )
}

fn str_lit(value: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value: value.into() } },
    )
}

#[test]
fn view_call_sites_unify_helper_params_and_stamp_signature() {
    let mut app = helper_app(bare_call("shout", str_lit("hi")));
    Analyzer::new(&app).analyze(&mut app);

    let helper = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "ApplicationHelper")
        .expect("helper LC");
    let shout = helper
        .methods
        .iter()
        .find(|m| m.name.as_str() == "shout")
        .expect("shout method");
    let Some(Ty::Fn { params, ret, .. }) = &shout.signature else {
        panic!("expected stamped Fn signature, got {:?}", shout.signature);
    };
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].ty, Ty::Str, "param should unify to Str from the call site");
    // Body is `msg` — the param read — so the return follows the seed.
    assert_eq!(**ret, Ty::Str);
}

#[test]
fn untyped_view_args_are_no_evidence_not_absorption() {
    // Two call sites: one typed Str, one whose arg the view typer
    // leaves untyped (an ivar with no controller seed dispatched
    // through an untyped read). The untyped site must not absorb the
    // union — the param stays Str.
    let typed = bare_call("shout", str_lit("hi"));
    let untyped_arg = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Ivar { name: Symbol::from("mystery") },
            )),
            method: Symbol::from("value"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    );
    let both = Expr::new(
        Span::synthetic(),
        ExprNode::Seq { exprs: vec![typed, bare_call("shout", untyped_arg)] },
    );
    let mut app = helper_app(both);
    Analyzer::new(&app).analyze(&mut app);

    let helper = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "ApplicationHelper")
        .expect("helper LC");
    let shout = helper
        .methods
        .iter()
        .find(|m| m.name.as_str() == "shout")
        .expect("shout method");
    let Some(Ty::Fn { params, .. }) = &shout.signature else {
        panic!("expected stamped Fn signature, got {:?}", shout.signature);
    };
    assert_eq!(
        params[0].ty,
        Ty::Str,
        "an untyped call site is no evidence; it must not widen the param: {:?}",
        params[0].ty
    );
}
