//! End-to-end: a synthetic App containing an `Int + Str` expression
//! produces the diagnostic the CLI would show, and an App without
//! such errors produces none.

use roundhouse::analyze::{diagnose, Analyzer, BodyTyper, Ctx};
use roundhouse::expr::{Expr, ExprNode, Literal};
use roundhouse::ident::Symbol;
use roundhouse::span::Span;
use roundhouse::App;

fn lit_int(v: i64) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit {
        value: Literal::Int { value: v },
    })
}

fn lit_str(s: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit {
        value: Literal::Str { value: s.to_string() },
    })
}

fn send(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Send {
        recv: Some(recv),
        method: Symbol::from(method),
        args,
        block: None,
        parenthesized: false,
    })
}

#[test]
fn diagnose_on_app_with_incompatible_binop_returns_one_diagnostic() {
    // Build an App whose seeds body is `1 + "hello"`. Seeds is a top-
    // level expression the analyzer walks, so it's the simplest seam
    // to stuff synthetic IR into without building a full controller.
    let mut app = App::new();
    let mut body = send(lit_int(1), "+", vec![lit_str("hello")]);

    // Hand-run the body-typer to populate .ty and detect the
    // Incompatible add annotation.
    let classes = std::collections::HashMap::new();
    let typer = BodyTyper::new(&classes);
    typer.analyze_expr(&mut body, &Ctx::default());
    app.seeds = Some(body);

    // Sanity: this is what the CLI's Analyzer::analyze + diagnose
    // pipeline would surface.
    Analyzer::new(&app).analyze(&mut app);
    let diags = diagnose(&app);

    assert!(
        !diags.is_empty(),
        "expected at least one diagnostic for `1 + \"hello\"`"
    );
    let incompat = diags
        .iter()
        .find(|d| d.code() == "incompatible_binop")
        .expect("incompatible_binop diagnostic should fire");
    assert!(
        incompat.message.contains("Int") && incompat.message.contains("Str"),
        "unexpected message: {}",
        incompat.message
    );
    assert!(incompat
        .to_string()
        .starts_with("error[incompatible_binop]: "));
}

#[test]
fn diagnose_on_clean_app_returns_empty() {
    // Default App has no controllers, models, etc. — the diagnose
    // walker visits nothing of concern. Empty list.
    let app = App::new();
    let diags = diagnose(&app);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
}
