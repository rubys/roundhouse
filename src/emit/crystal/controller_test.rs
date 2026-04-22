//! Crystal controller-test emit (uses shared classifier). Builds the
//! `it "..." do …` blocks for `*_controller_spec.cr` files via
//! `emit_crystal_spec` in `spec.rs`.

use std::fmt::Write;

use super::expr::emit_literal;
use crate::App;
use crate::dialect::Test;
use crate::expr::{Expr, ExprNode, LValue, Literal};

// --- Crystal controller-test emit (uses shared classifier) ----------

pub(super) fn emit_cr_controller_test(out: &mut String, test: &Test, app: &App) {
    writeln!(out, "  it {:?} do", test.name.as_str()).unwrap();
    writeln!(out, "    client = Roundhouse::TestSupport::TestClient.new").unwrap();

    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(&crate::naming::camelize(ivar.as_str()));
        let fixture_mod = crate::naming::camelize(&plural);
        writeln!(
            out,
            "    {} = Fixtures::{}.one",
            ivar.as_str(),
            fixture_mod,
        )
        .unwrap();
    }

    let stmts = crate::lower::test_body_stmts(&test.body);
    for stmt in stmts {
        let rendered = emit_cr_ctrl_test_stmt(stmt, app);
        for line in rendered.lines() {
            writeln!(out, "    {line}").unwrap();
        }
    }

    writeln!(out, "  end").unwrap();
}

fn emit_cr_ctrl_test_stmt(stmt: &Expr, app: &App) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_cr_ctrl_test_send(method.as_str(), args, block.as_ref(), app)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_cr_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.reload");
            }
            let recv_s = emit_cr_ctrl_test_expr(r, app);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{name} = {}", emit_cr_ctrl_test_expr(value, app))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("{name} = {}", emit_cr_ctrl_test_expr(value, app))
        }
        _ => emit_cr_ctrl_test_expr(stmt, app),
    }
}

fn emit_cr_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_cr_url_expr(url, app);
            format!("resp = client.get({u})")
        }
        Some(ControllerTestSend::HttpWrite { method, url, params }) => {
            let u = emit_cr_url_expr(url, app);
            let body = params
                .map(|h| flatten_cr_params_to_form(h, None, app))
                .unwrap_or_else(|| "{} of String => String".to_string());
            format!("resp = client.{method}({u}, {body})")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_cr_url_expr(url, app);
            format!("resp = client.delete({u})")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "resp.assert_ok".to_string(),
            "unprocessable_entity" => "resp.assert_unprocessable".to_string(),
            other => format!("resp.assert_status(200) # TODO {other:?}"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_cr_url_expr(url, app);
            format!("resp.assert_redirected_to({u})")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_cr_assert_select(selector, kind, app)
        }
        Some(ControllerTestSend::AssertDifference { method, count_expr, delta, block }) => {
            let _ = method;
            emit_cr_assert_difference(count_expr, delta, block, app)
        }
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_cr_ctrl_test_expr(expected, app);
            let a = emit_cr_ctrl_test_expr(actual, app);
            format!("({a}).should eq({e})")
        }
        None => {
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
    }
}

fn emit_cr_url_expr(expr: &Expr, app: &App) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_cr_ctrl_test_expr(expr, app);
    };
    let helper_name = format!("{}_path", helper.helper_base);
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}.last.not_nil!.id", class.as_str()),
            UrlArg::Raw(e) => emit_cr_ctrl_test_expr(e, app),
        })
        .collect();
    format!("RouteHelpers.{helper_name}({})", args_s.join(", "))
}

fn emit_cr_assert_select(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } = &*selector_expr.node
    else {
        return format!(
            "# TODO: dynamic selector\nresp.assert_select({})",
            emit_cr_ctrl_test_expr(selector_expr, app),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_cr_ctrl_test_expr(expr, app);
            format!("resp.assert_select_text({selector:?}, {text})")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_cr_ctrl_test_expr(expr, app);
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
                out.push_str(&emit_cr_ctrl_test_stmt(stmt, app));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("resp.assert_select({selector:?})")
        }
    }
}

fn emit_cr_assert_difference(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
) -> String {
    // "Article.count" → `Article.count` (already valid Crystal).
    let count_expr = count_expr_str.clone();

    let mut out = String::new();
    out.push_str(&format!("_before = {count_expr}\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in crate::lower::test_body_stmts(inner_body) {
            out.push_str(&emit_cr_ctrl_test_stmt(stmt, app));
            out.push('\n');
        }
    }
    out.push_str(&format!("_after = {count_expr}\n"));
    out.push_str(&format!("(_after - _before).should eq({expected_delta})"));
    out
}

fn emit_cr_ctrl_test_expr(expr: &Expr, app: &App) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => path
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.last.not_nil!");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.count");
                }
            }
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_cr_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_cr_ctrl_test_expr(r, app);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_ctrl_test_expr(a, app)).collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_cr_url_expr(expr, app);
            }
            let args_s: Vec<String> =
                args.iter().map(|a| emit_cr_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        _ => format!("# TODO expr {:?}", std::mem::discriminant(&*expr.node)),
    }
}

fn flatten_cr_params_to_form(expr: &Expr, scope: Option<&str>, app: &App) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_cr_ctrl_test_expr(value, app);
            format!("{key:?} => {val}.to_s")
        })
        .collect();
    if pairs.is_empty() {
        return "{} of String => String".to_string();
    }
    format!("{{ {} }} of String => String", pairs.join(", "))
}
