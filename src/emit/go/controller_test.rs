//! Pass-2 controller-test emit.
//!
//! Walks each controller-test body via the shared `test_body_stmts`
//! helper and dispatches each statement through
//! `classify_controller_test_send` + a Go render table. Mirrors the
//! Python and Elixir emitters; differences are Go's static typing
//! (TestClient takes `*testing.T`; assertion methods don't take a
//! receiver capture).

use crate::App;
use crate::dialect::Test;
use crate::expr::{Expr, ExprNode, LValue, Literal};

use super::expr::emit_literal;
use super::shared::{go_field_name, go_method_name, pascalize_word};

#[derive(Clone, Copy)]
pub(super) struct GoTestCtx<'a> {
    pub(super) app: &'a App,
    pub(super) fixture_names: &'a [crate::ident::Symbol],
    pub(super) known_models: &'a [crate::ident::Symbol],
    pub(super) model_attrs: &'a [crate::ident::Symbol],
}

pub(super) fn emit_go_controller_test_body(
    test: &Test,
    app: &App,
    ctx: &GoTestCtx,
) -> String {
    let mut out = String::new();
    out.push_str("client := NewTestClient(t)\n");
    // Prime ivars read without assignment via fixture lookup:
    // `@article` → `article := ArticlesOne()`.
    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(ivar.as_str());
        if ctx.fixture_names.iter().any(|s| s.as_str() == plural) {
            out.push_str(&format!(
                "{} := {}{}()\n",
                ivar.as_str(),
                pascalize_word(&plural),
                pascalize_word("one"),
            ));
        }
    }
    // Acknowledge `client` so Go doesn't error if the test only uses
    // assertion-style helpers; in practice every classified test uses
    // the client at least once, but the fall-through path needs the
    // safety belt.
    out.push_str("_ = client\n");
    for stmt in crate::lower::test_body_stmts(&test.body) {
        let rendered = emit_go_ctrl_test_stmt(stmt, app, ctx);
        out.push_str(&rendered);
        out.push('\n');
    }
    out
}

fn emit_go_ctrl_test_stmt(stmt: &Expr, app: &App, ctx: &GoTestCtx) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_go_ctrl_test_send(method.as_str(), args, block.as_ref(), app, ctx)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => {
                        name.to_string()
                    }
                    _ => emit_go_ctrl_test_expr(r, app, ctx),
                };
                return format!("{recv_s}.Reload()");
            }
            let recv_s = emit_go_ctrl_test_expr(r, app, ctx);
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_go_ctrl_test_expr(a, app, ctx))
                .collect();
            if args_s.is_empty() {
                // Field access — Go uses bare-name access for struct
                // fields, no parens.
                format!("{recv_s}.{}", go_field_name(method.as_str()))
            } else {
                format!(
                    "{recv_s}.{}({})",
                    go_method_name(method.as_str()),
                    args_s.join(", ")
                )
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{name} := {}", emit_go_ctrl_test_expr(value, app, ctx))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("{name} := {}", emit_go_ctrl_test_expr(value, app, ctx))
        }
        _ => emit_go_ctrl_test_expr(stmt, app, ctx),
    }
}

fn emit_go_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
    ctx: &GoTestCtx,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_go_url_expr(url, app, ctx);
            format!("resp := client.Get({u})")
        }
        Some(ControllerTestSend::HttpWrite { method, url, params }) => {
            let u = emit_go_url_expr(url, app, ctx);
            let body = params
                .map(|h| flatten_go_params_to_form(h, None, app, ctx))
                .unwrap_or_else(|| "map[string]string{}".to_string());
            let go_method = match method {
                "post" => "Post",
                "patch" => "Patch",
                "put" => "Put",
                _ => "Post",
            };
            format!("resp := client.{go_method}({u}, {body})")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_go_url_expr(url, app, ctx);
            format!("resp := client.Delete({u})")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "resp.AssertOk()".to_string(),
            "unprocessable_entity" => "resp.AssertUnprocessable()".to_string(),
            other => format!("resp.AssertStatus(200) // TODO: {other:?}"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_go_url_expr(url, app, ctx);
            format!("resp.AssertRedirectedTo({u})")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_go_assert_select_classified(selector, kind, app, ctx)
        }
        Some(ControllerTestSend::AssertDifference {
            method: _,
            count_expr,
            delta,
            block,
        }) => emit_go_assert_difference_classified(count_expr, delta, block, app, ctx),
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_go_ctrl_test_expr(expected, app, ctx);
            let a = emit_go_ctrl_test_expr(actual, app, ctx);
            format!(
                "if {a} != {e} {{ t.Errorf(\"expected %v, got %v\", {e}, {a}) }}"
            )
        }
        None => {
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_go_ctrl_test_expr(a, app, ctx))
                .collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
    }
}

fn emit_go_url_expr(expr: &Expr, app: &App, ctx: &GoTestCtx) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_go_ctrl_test_expr(expr, app, ctx);
    };
    let helper_name = format!("{}Path", go_field_name(&helper.helper_base));
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.ID"),
            UrlArg::ModelLast(class) => format!("{}Last().ID", class.as_str()),
            UrlArg::Raw(e) => emit_go_ctrl_test_expr(e, app, ctx),
        })
        .collect();
    format!("{helper_name}({})", args_s.join(", "))
}

fn emit_go_assert_select_classified(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
    ctx: &GoTestCtx,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } =
        &*selector_expr.node
    else {
        return format!(
            "resp.AssertSelect({}) // TODO: dynamic selector",
            emit_go_ctrl_test_expr(selector_expr, app, ctx),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_go_ctrl_test_expr(expr, app, ctx);
            format!("resp.AssertSelectText({selector:?}, {text})")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_go_ctrl_test_expr(expr, app, ctx);
            format!("resp.AssertSelectMin({selector:?}, int({n}))")
        }
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = format!("resp.AssertSelect({selector:?})\n");
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in crate::lower::test_body_stmts(inner_body) {
                out.push_str(&emit_go_ctrl_test_stmt(stmt, app, ctx));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("resp.AssertSelect({selector:?})")
        }
    }
}

fn emit_go_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
    ctx: &GoTestCtx,
) -> String {
    // `Article.count` → `ArticleCount()`. Mirrors the Go test send
    // rewrite for class-method calls.
    let count_expr = count_expr_str
        .split_once('.')
        .map(|(cls, m)| format!("{cls}{}()", pascalize_word(m)))
        .unwrap_or_else(|| count_expr_str.clone());
    let mut out = String::new();
    out.push_str(&format!("countBefore := {count_expr}\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in crate::lower::test_body_stmts(inner_body) {
            out.push_str(&emit_go_ctrl_test_stmt(stmt, app, ctx));
            out.push('\n');
        }
    }
    out.push_str(&format!("countAfter := {count_expr}\n"));
    out.push_str(&format!(
        "if countAfter - countBefore != {expected_delta} {{ t.Errorf(\"expected delta {expected_delta}, got %d\", countAfter - countBefore) }}",
    ));
    out
}

fn emit_go_ctrl_test_expr(expr: &Expr, app: &App, ctx: &GoTestCtx) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            // Class-level helpers: `Class.last`, `Class.count` →
            // package-level `ClassLast()` / `ClassCount()`.
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path
                        .last()
                        .map(|s| s.as_str().to_string())
                        .unwrap_or_default();
                    return format!("{class}Last()");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path
                        .last()
                        .map(|s| s.as_str().to_string())
                        .unwrap_or_default();
                    return format!("{class}Count()");
                }
            }
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => {
                        name.to_string()
                    }
                    _ => emit_go_ctrl_test_expr(r, app, ctx),
                };
                return format!("{recv_s}.{}", go_field_name(m));
            }
            let recv_s = emit_go_ctrl_test_expr(r, app, ctx);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_go_ctrl_test_expr(a, app, ctx)).collect();
            format!(
                "{recv_s}.{}({})",
                go_method_name(m),
                args_s.join(", ")
            )
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_go_url_expr(expr, app, ctx);
            }
            let args_s: Vec<String> =
                args.iter().map(|a| emit_go_ctrl_test_expr(a, app, ctx)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        _ => "nil /* TODO expr */".to_string(),
    }
}

/// Flatten `{ article: { title: "X", body: "Y" } }` into a Go map
/// literal `map[string]string{"article[title]": "X", "article[body]":
/// "Y"}` matching the TestClient form-body shape.
fn flatten_go_params_to_form(
    expr: &Expr,
    scope: Option<&str>,
    app: &App,
    ctx: &GoTestCtx,
) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let v = emit_go_ctrl_test_expr(value, app, ctx);
            // Coerce non-string values via fmt.Sprintf so the
            // map[string]string form-body is satisfied.
            format!("{key:?}: fmt.Sprint({v})")
        })
        .collect();
    format!("map[string]string{{{}}}", pairs.join(", "))
}
