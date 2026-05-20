//! Controller-test emit — relocated from `src/emit/rust/spec.rs`
//! during Phase 7.2. The rust2 path filters `app.test_modules` to
//! `ActionDispatch::IntegrationTest`-parented modules (everything
//! else takes the lowerer-based model-test path in `rust2.rs`),
//! so this file only carries the controller-test rendering — the
//! legacy spec.rs's model-test branch isn't reachable from here.
//!
//! Each emitted `src/tests/<snake>.rs` file gets one
//! `#[tokio::test(flavor = "multi_thread")]` async fn per Ruby
//! `test "..."` declaration. Bodies translate to axum-test +
//! `TestResponseExt` calls through the shared classifiers in
//! `crate::lower::{classify_controller_test_send, classify_url_expr,
//! flatten_params_pairs}`.

use std::fmt::Write;
use std::path::PathBuf;

use crate::App;
use crate::dialect::{Test, TestModule};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::naming::snake_case;

use super::super::EmittedFile;

/// `src/emit/rust2/expr/literal.rs::emit_literal` is the rust2 literal
/// helper; alias for clarity at this site.
use super::expr::util as _expr_util;

fn emit_literal_local(lit: &Literal) -> String {
    super::expr::literal::emit_literal(lit)
}

/// Controller-test renderer. Walks a Minitest body and emits each
/// statement to axum-test + assertion-trait shapes.
///
/// Covers the scaffold blog's assertions:
///   - HTTP verbs: `get` / `post` / `patch` / `delete`
///   - Status: `assert_response :success | :unprocessable_entity`
///   - Redirects: `assert_redirected_to <url>`
///   - Structural: `assert_select <sel>[, text]` + nested block +
///     `minimum: N`
///   - Count: `assert_difference(<expr>[, <delta>]) { body }` +
///     `assert_no_difference`
///   - Equality: `assert_equal a, b`
///   - Model: `Model.last`, `@record.reload`
///
/// Setup (`setup do @article = articles(:one) end`) isn't preserved
/// in the current IR; ivars read-without-assign auto-prime from the
/// `<plural>::one()` fixture accessor. Matches real-blog's convention.
fn emit_rust_controller_test(out: &mut String, test: &Test, app: &App) {
    let name = test_fn_name(&test.name);
    writeln!(out, "#[tokio::test(flavor = \"multi_thread\")]").unwrap();
    writeln!(out, "#[allow(unused_mut, unused_variables)]").unwrap();
    writeln!(out, "async fn {name}() {{").unwrap();
    writeln!(out, "    // {:?}", test.name).unwrap();
    writeln!(out, "    fixtures::setup();").unwrap();
    writeln!(
        out,
        "    let server = axum_test::TestServer::new(crate::router::router()).unwrap();",
    )
    .unwrap();

    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(&crate::naming::camelize(ivar.as_str()));
        writeln!(
            out,
            "    let mut {} = fixtures::{}::one();",
            ivar.as_str(),
            plural,
        )
        .unwrap();
    }

    let stmts = ctrl_test_body_stmts(&test.body);
    for stmt in stmts {
        let rendered = emit_ctrl_test_stmt(stmt, app);
        for line in rendered.lines() {
            writeln!(out, "    {line}").unwrap();
        }
    }

    writeln!(out, "}}").unwrap();
}

fn ctrl_test_body_stmts(body: &Expr) -> Vec<&Expr> {
    crate::lower::test_body_stmts(body)
}

fn emit_ctrl_test_stmt(stmt: &Expr, app: &App) -> String {
    use crate::expr::LValue;
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_ctrl_test_send(method.as_str(), args, block.as_ref(), app)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } => name.to_string(),
                    ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.reload();");
            }
            let recv_s = emit_ctrl_test_expr(r, app);
            let args_s: Vec<String> = args.iter().map(|a| emit_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{recv_s}.{method}();")
            } else {
                format!("{recv_s}.{method}({});", args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("let mut {name} = {};", emit_ctrl_test_expr(value, app))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("let mut {name} = {};", emit_ctrl_test_expr(value, app))
        }
        _ => format!("{};", emit_ctrl_test_expr(stmt, app)),
    }
}

fn emit_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_url_expr(url, app);
            format!("let resp = server.get(&{u}).await;")
        }
        Some(ControllerTestSend::HttpWrite { method, url, params }) => {
            let u = emit_url_expr(url, app);
            let form_body = params
                .map(|h| flatten_params_to_form(h, None, app))
                .unwrap_or_else(|| "std::collections::HashMap::<String, String>::new()".to_string());
            format!("let resp = server.{method}(&{u}).form(&{form_body}).await;")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_url_expr(url, app);
            format!("let resp = server.delete(&{u}).await;")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "resp.assert_ok();".to_string(),
            "unprocessable_entity" | "unprocessable_content" => {
                "resp.assert_unprocessable();".to_string()
            }
            other => format!("resp.assert_status(/* {other:?} */ 200);"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_url_expr(url, app);
            format!("resp.assert_redirected_to(&{u});")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_assert_select_classified(selector, kind, app)
        }
        Some(ControllerTestSend::AssertDifference { method, count_expr, delta, block }) => {
            let _ = method;
            emit_assert_difference_classified(count_expr, delta, block, app)
        }
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_ctrl_test_expr(expected, app);
            let a = emit_ctrl_test_expr(actual, app);
            format!("assert_eq!({e}, {a});")
        }
        None => {
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{method}();")
            } else {
                format!("{method}({});", args_s.join(", "))
            }
        }
    }
}

fn flatten_params_to_form(expr: &Expr, scope: Option<&str>, app: &App) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_ctrl_test_expr(value, app);
            format!("({key:?}.to_string(), {val}.to_string())")
        })
        .collect();
    format!(
        "std::collections::HashMap::<String, String>::from([{}])",
        pairs.join(", "),
    )
}

fn emit_url_expr(expr: &Expr, app: &App) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_ctrl_test_expr(expr, app);
    };
    let helper_name = format!("{}_path", helper.helper_base);
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}::last().unwrap().id", class.as_str()),
            UrlArg::Raw(e) => emit_ctrl_test_expr(e, app),
        })
        .collect();
    format!("route_helpers::{helper_name}({})", args_s.join(", "))
}

fn emit_assert_select_classified(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } = &*selector_expr.node
    else {
        return format!(
            "/* TODO: dynamic selector */ resp.assert_select({:?});",
            emit_ctrl_test_expr(selector_expr, app),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_ctrl_test_expr(expr, app);
            format!("resp.assert_select_text({selector:?}, &{text});")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_ctrl_test_expr(expr, app);
            format!("resp.assert_select_min({selector:?}, {n} as usize);")
        }
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = String::new();
            out.push_str(&format!("resp.assert_select({selector:?});\n"));
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in ctrl_test_body_stmts(inner_body) {
                out.push_str(&emit_ctrl_test_stmt(stmt, app));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("resp.assert_select({selector:?});")
        }
    }
}

fn emit_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
) -> String {
    let count_expr = count_expr_str
        .split_once('.')
        .map(|(cls, m)| format!("{cls}::{m}()"))
        .unwrap_or_else(|| count_expr_str.clone());

    let mut out = String::new();
    out.push_str(&format!("let _before = {count_expr};\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in ctrl_test_body_stmts(inner_body) {
            out.push_str(&emit_ctrl_test_stmt(stmt, app));
            out.push('\n');
        }
    }
    out.push_str(&format!("let _after = {count_expr};\n"));
    out.push_str(&format!("assert_eq!(_after - _before, {expected_delta});"));
    out
}

fn emit_ctrl_test_expr(expr: &Expr, app: &App) -> String {
    let _ = app;
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal_local(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}::last().unwrap()");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}::count()");
                }
            }
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_ctrl_test_expr(r, app);
            let args_s: Vec<String> = args.iter().map(|a| emit_ctrl_test_expr(a, app)).collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_url_expr(expr, app);
            }
            let args_s: Vec<String> = args.iter().map(|a| emit_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        _ => format!("/* TODO expr {:?} */", std::mem::discriminant(&*expr.node)),
    }
}

/// Per-module emit entry. Renders one `#[tokio::test]` async fn per
/// Ruby `test "..."` declaration in `tm` to a `src/tests/<snake>.rs`
/// file. The rust2 caller (`src/emit/rust2.rs`) filters to
/// IntegrationTest-parented modules before calling this — so every
/// `Test` in `tm.tests` is a controller test.
pub(super) fn emit_rust_test_module(tm: &TestModule, app: &App) -> EmittedFile {
    let _ = _expr_util::synth_default_for_ty; // bind the alias

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "#[allow(unused_imports)]").unwrap();
    writeln!(s, "use crate::fixtures;").unwrap();
    writeln!(s, "#[allow(unused_imports)]").unwrap();
    writeln!(s, "use crate::models::*;").unwrap();
    writeln!(s, "#[allow(unused_imports)]").unwrap();
    writeln!(s, "use crate::route_helpers;").unwrap();
    writeln!(s, "#[allow(unused_imports)]").unwrap();
    writeln!(s, "use crate::test_support::TestResponseExt;").unwrap();

    // Avoid unused-warning on the symbol collectors (we read from
    // `app.fixtures` / `app.models` directly inside the per-test
    // emit, but the top-level scan exists to keep parity with the
    // legacy emit header in case future generic-test-header logic
    // needs the symbol set).
    let _: Vec<Symbol> = app.fixtures.iter().map(|f| f.name.clone()).collect();
    let _: Vec<Symbol> = app.models.iter().map(|m| m.name.0.clone()).collect();

    for test in &tm.tests {
        writeln!(s).unwrap();
        emit_rust_controller_test(&mut s, test, app);
    }

    let filename = snake_case(tm.name.0.as_str());
    EmittedFile {
        path: PathBuf::from(format!("src/tests/{filename}.rs")),
        content: s,
    }
}

fn test_fn_name(desc: &str) -> String {
    let mut s: String = desc
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_matches('_').to_string()
}
