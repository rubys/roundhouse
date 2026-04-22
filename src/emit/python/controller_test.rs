//! Python controller-test emit (uses shared classifier). Builds the
//! `def test_*` body for `tests/test_*_controller.py` files via
//! `emit_py_test` in `spec.rs`.

use super::expr::emit_literal;
use crate::App;
use crate::dialect::Test;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;

#[derive(Clone, Copy)]
pub(super) struct PyTestCtx<'a> {
    pub(super) app: &'a App,
    pub(super) fixture_names: &'a [Symbol],
    pub(super) known_models: &'a [Symbol],
    pub(super) model_attrs: &'a [Symbol],
}

/// Emit a single controller test body. Walks the test AST via the
/// shared `test_body_stmts` helper and dispatches each statement
/// through the shared `classify_controller_test_send` classifier
/// plus a Python render table.
pub(super) fn emit_py_controller_test_body(test: &Test, app: &App, ctx: &PyTestCtx) -> String {
    let mut out = String::new();
    out.push_str("client = TestClient()\n");
    // Prime ivars read without assignment: `@article` → `article = fixtures.articles_one()`.
    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        // Match convention: `@article` with fixture prefix `articles`
        // binds to `fixtures.articles_one()`. The fixture_names in ctx
        // are plural fixture names.
        let plural = crate::naming::pluralize_snake(ivar.as_str());
        if ctx.fixture_names.iter().any(|s| s.as_str() == plural) {
            out.push_str(&format!(
                "{} = fixtures.{}_one()\n",
                ivar.as_str(),
                plural,
            ));
        }
    }

    for stmt in crate::lower::test_body_stmts(&test.body) {
        let rendered = emit_py_ctrl_test_stmt(stmt, app, ctx);
        out.push_str(&rendered);
        out.push('\n');
    }
    out
}

fn emit_py_ctrl_test_stmt(stmt: &Expr, app: &App, ctx: &PyTestCtx) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_py_ctrl_test_send(method.as_str(), args, block.as_ref(), app, ctx)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_py_ctrl_test_expr(r, app, ctx),
                };
                return format!("{recv_s}.reload()");
            }
            let recv_s = emit_py_ctrl_test_expr(r, app, ctx);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_py_ctrl_test_expr(a, app, ctx)).collect();
            if args_s.is_empty() {
                // Attribute read vs method call — for controller
                // tests we always parens (simpler than tracking).
                format!("{recv_s}.{method}()")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{name} = {}", emit_py_ctrl_test_expr(value, app, ctx))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("{name} = {}", emit_py_ctrl_test_expr(value, app, ctx))
        }
        _ => emit_py_ctrl_test_expr(stmt, app, ctx),
    }
}

fn emit_py_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
    ctx: &PyTestCtx,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_py_url_expr(url, app, ctx);
            format!("resp = client.get({u})")
        }
        Some(ControllerTestSend::HttpWrite { method, url, params }) => {
            let u = emit_py_url_expr(url, app, ctx);
            let body = params
                .map(|h| flatten_py_params_to_form(h, None, app, ctx))
                .unwrap_or_else(|| "{}".to_string());
            format!("resp = client.{method}({u}, {body})")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_py_url_expr(url, app, ctx);
            format!("resp = client.delete({u})")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "resp.assert_ok()".to_string(),
            "unprocessable_entity" => "resp.assert_unprocessable()".to_string(),
            other => format!("resp.assert_status(200)  # TODO: {other:?}"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_py_url_expr(url, app, ctx);
            format!("resp.assert_redirected_to({u})")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_py_assert_select_classified(selector, kind, app, ctx)
        }
        Some(ControllerTestSend::AssertDifference { method: _, count_expr, delta, block }) => {
            emit_py_assert_difference_classified(count_expr, delta, block, app, ctx)
        }
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_py_ctrl_test_expr(expected, app, ctx);
            let a = emit_py_ctrl_test_expr(actual, app, ctx);
            format!("self.assertEqual({a}, {e})")
        }
        None => {
            let args_s: Vec<String> =
                args.iter().map(|a| emit_py_ctrl_test_expr(a, app, ctx)).collect();
            if args_s.is_empty() {
                format!("{method}()")
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
    }
}

fn emit_py_url_expr(expr: &Expr, app: &App, ctx: &PyTestCtx) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_py_ctrl_test_expr(expr, app, ctx);
    };
    let helper_name = format!("{}_path", helper.helper_base);
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}.last().id", class.as_str()),
            UrlArg::Raw(e) => emit_py_ctrl_test_expr(e, app, ctx),
        })
        .collect();
    format!("{helper_name}({})", args_s.join(", "))
}

fn emit_py_assert_select_classified(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
    ctx: &PyTestCtx,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } =
        &*selector_expr.node
    else {
        return format!(
            "resp.assert_select({})  # TODO: dynamic selector",
            emit_py_ctrl_test_expr(selector_expr, app, ctx),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_py_ctrl_test_expr(expr, app, ctx);
            format!("resp.assert_select_text({selector:?}, {text})")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_py_ctrl_test_expr(expr, app, ctx);
            format!("resp.assert_select_min({selector:?}, {n})")
        }
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = String::new();
            out.push_str(&format!("resp.assert_select({selector:?})\n"));
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in crate::lower::test_body_stmts(inner_body) {
                out.push_str(&emit_py_ctrl_test_stmt(stmt, app, ctx));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("resp.assert_select({selector:?})")
        }
    }
}

fn emit_py_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
    ctx: &PyTestCtx,
) -> String {
    // `Article.count` → `Article.count()` in Python.
    let count_expr = count_expr_str
        .split_once('.')
        .map(|(cls, m)| format!("{cls}.{m}()"))
        .unwrap_or_else(|| count_expr_str.clone());

    let mut out = String::new();
    out.push_str(&format!("_before = {count_expr}\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in crate::lower::test_body_stmts(inner_body) {
            out.push_str(&emit_py_ctrl_test_stmt(stmt, app, ctx));
            out.push('\n');
        }
    }
    out.push_str(&format!("_after = {count_expr}\n"));
    out.push_str(&format!("self.assertEqual(_after - _before, {expected_delta})"));
    out
}

fn emit_py_ctrl_test_expr(expr: &Expr, app: &App, ctx: &PyTestCtx) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.last()");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.count()");
                }
            }
            if args.is_empty() {
                // Attribute access for ivar.attr / var.attr shapes;
                // method call otherwise. Python has no null-coalescing
                // the way TS does, so use attribute by default.
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_py_ctrl_test_expr(r, app, ctx),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_py_ctrl_test_expr(r, app, ctx);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_py_ctrl_test_expr(a, app, ctx)).collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_py_url_expr(expr, app, ctx);
            }
            let args_s: Vec<String> =
                args.iter().map(|a| emit_py_ctrl_test_expr(a, app, ctx)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        _ => format!("None  # TODO expr"),
    }
}

/// Flatten `{ article: { title: "X", body: "Y" } }` into a Python
/// dict literal `{ "article[title]": "X", "article[body]": "Y" }`
/// — matching the TestClient's form-body shape.
fn flatten_py_params_to_form(
    expr: &Expr,
    scope: Option<&str>,
    app: &App,
    ctx: &PyTestCtx,
) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_py_ctrl_test_expr(value, app, ctx);
            format!("{key:?}: str({val})")
        })
        .collect();
    format!("{{{}}}", pairs.join(", "))
}
