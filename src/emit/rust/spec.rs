//! Test modules — emit one Rust test module per Ruby test file, plus
//! the `src/tests/mod.rs` that declares them. Controller tests go
//! through a dedicated renderer (`emit_rust_controller_test`) that
//! targets axum-test + the hand-written `TestResponseExt` trait;
//! model tests reuse the shared `emit_body` from `controller.rs`.

use std::fmt::Write;
use std::path::PathBuf;

use crate::App;
use crate::dialect::{Test, TestModule};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::naming::snake_case;

use super::super::EmittedFile;
use super::controller::{emit_body, EmitCtx};
use super::shared::emit_literal;

/// Emit a `src/tests/<snake>.rs` file containing one `#[test] fn` per
/// Ruby `test "..."` declaration in the source test module. Test names
/// are snake-cased from the Ruby description string. Bodies are rendered
/// with test-context emit enabled (fixture accessors, assertion mapping,
/// struct-literal `Class.new`).
/// Phase 4d controller-test emit. Walks a Rails Minitest body and
/// renders each statement to the axum-test + TestResponseExt shape.
/// Fully pattern-matched — doesn't reuse the SendKind classifier
/// because test-body shapes (`assert_response`, `assert_select`,
/// `get <url>`, etc.) are distinct from controller-body shapes and
/// not shared with other targets.
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
/// in the current IR, so ivars read-without-assign get auto-primed
/// from the fixtures' `one` entry. Matches real-blog's convention.
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

    // Prime each ivar the body reads but doesn't assign, from the
    // `<plural>::one()` fixture accessor. Same convention as Rails'
    // scaffold `setup` block.
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

/// Flatten a test body into a statement sequence. If the body is a
/// single Seq, unwrap it; otherwise return a singleton.
fn ctrl_test_body_stmts(body: &Expr) -> Vec<&Expr> {
    crate::lower::test_body_stmts(body)
}

/// Emit a single controller-test statement.
fn emit_ctrl_test_stmt(stmt: &Expr, app: &App) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_ctrl_test_send(method.as_str(), args, block.as_ref(), app)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            // Instance method calls — primarily `@record.reload`.
            if method.as_str() == "reload" {
                // Ivar receivers rendered bare (the ivar priming
                // above bound them as locals).
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

/// Top-level Send dispatcher for test body statements. Recognizes
/// Minitest + Rails test primitives via the shared classifier and
/// renders each variant per Rust's axum_test conventions. Unknown
/// shapes fall back to a best-effort `method(args)` render.
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
            "unprocessable_entity" => "resp.assert_unprocessable();".to_string(),
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
            // Rails calls assert_equal(expected, actual); match
            // Rust's assert_eq! argument order.
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

/// Flatten a Ruby-shape params Hash into a Rust `HashMap<String,
/// String>` literal matching Rails' bracketed-key form. Delegates
/// key-flattening to `crate::lower::flatten_params_pairs`; this
/// function is just the Rust-side value render.
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

/// Render a URL-helper call (`articles_url`, `article_url(@article)`)
/// into a `route_helpers::*_path(...)` call returning `String`. Uses
/// the shared URL-helper classifier — Rust-specific pieces are the
/// `_path` suffix and the `Model::last().unwrap().id` unwrap syntax.
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

/// `assert_select` render over the shared classifier. Rust-specific
/// pieces: `&` borrow on string args, `as usize` cast on the
/// minimum-count arg.
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
        // Block form: `assert_select "#articles" do assert_select "h2",
        // minimum: 1 end`. Outer selector check + recurse through the
        // block body as parallel assertions (no nested scoping).
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

/// `assert_difference(<expr>[, <delta>]) { body }` — render with
/// Rust-specific `Model::count()` syntax. Delta + block come
/// pre-classified.
fn emit_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
) -> String {
    // Rewrite "Article.count" → "Article::count()".
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

/// Expression-level emit for test bodies — literals, ivar reads, a
/// few targeted call rewrites (`Article.last`, `Article.count`).
/// Doesn't try to be general; unknown shapes fall through to a
/// stringified approximation.
fn emit_ctrl_test_expr(expr: &Expr, app: &App) -> String {
    let _ = app;
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            // `Model.last` → `Model::last().unwrap()`.
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}::last().unwrap()");
                }
            }
            // `Model.count` → `Model::count()`.
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}::count()");
                }
            }
            // Attribute read on ivar/var (`@article.title` →
            // `article.title`).
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
            // Bare fn call — probably a route helper.
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

pub(super) fn emit_rust_test_module(tm: &TestModule, app: &App) -> EmittedFile {
    let fixture_names: Vec<Symbol> =
        app.fixtures.iter().map(|f| f.name.clone()).collect();
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    // Flat union of attribute names across every model. Dedup so the
    // slice stays compact; collisions on common names (id, body, etc.)
    // are expected and fine.
    let mut attrs_set: std::collections::BTreeSet<Symbol> =
        std::collections::BTreeSet::new();
    for m in &app.models {
        for attr in m.attributes.fields.keys() {
            attrs_set.insert(attr.clone());
        }
    }
    let model_attrs: Vec<Symbol> = attrs_set.into_iter().collect();

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "#[allow(unused_imports)]").unwrap();
    writeln!(s, "use crate::fixtures;").unwrap();
    writeln!(s, "#[allow(unused_imports)]").unwrap();
    writeln!(s, "use crate::models::*;").unwrap();
    // Controller-test modules additionally reference route helpers,
    // axum-test, and the test-support assertion trait. Extra imports
    // land conditionally so model tests don't pull in axum deps.
    let is_ctrl_test_header = tm.name.0.as_str().ends_with("ControllerTest");
    if is_ctrl_test_header {
        writeln!(s, "#[allow(unused_imports)]").unwrap();
        writeln!(s, "use crate::route_helpers;").unwrap();
        writeln!(s, "#[allow(unused_imports)]").unwrap();
        writeln!(s, "use crate::test_support::TestResponseExt;").unwrap();
    }

    let ctx = EmitCtx {
        self_methods: &[],
        in_test: true,
        in_controller: false,
        fixture_names: &fixture_names,
        known_models: &known_models,
        model_attrs: &model_attrs,
        app: Some(app),
    };

    let is_controller_test = tm.name.0.as_str().ends_with("ControllerTest");
    for test in &tm.tests {
        writeln!(s).unwrap();
        if is_controller_test {
            emit_rust_controller_test(&mut s, test, app);
        } else if test_needs_runtime_unsupported(test) {
            // Body would either fail to compile (destroy/count/
            // assert_difference) or fail at run time (save returning
            // true where a DB check would have made it false).
            // Emit with #[ignore] and a short TODO so the test count
            // stays visible in `cargo test` output.
            writeln!(s, "#[test]").unwrap();
            writeln!(s, "#[ignore] // Phase 3: needs persistence runtime").unwrap();
            writeln!(s, "fn {}() {{", test_fn_name(&test.name)).unwrap();
            writeln!(s, "    // {:?}", test.name).unwrap();
            writeln!(s, "    // TODO: requires save/destroy/aggregate support").unwrap();
            writeln!(s, "}}").unwrap();
        } else {
            writeln!(s, "#[test]").unwrap();
            // Test bodies emit `let mut` uniformly so save/destroy
            // calls on model bindings type-check; this allow-attr
            // silences the resulting unused-mut warnings on bindings
            // that never actually mutate.
            writeln!(s, "#[allow(unused_mut)]").unwrap();
            writeln!(s, "fn {}() {{", test_fn_name(&test.name)).unwrap();
            // Every test starts on a fresh :memory: DB with all
            // fixtures loaded. `setup` is idempotent across repeat
            // calls on the same thread, so a prior test's state
            // never leaks in.
            if !app.fixtures.is_empty() {
                writeln!(s, "    crate::fixtures::setup();").unwrap();
            }
            for line in emit_body(&test.body, ctx).lines() {
                writeln!(s, "    {line}").unwrap();
            }
            writeln!(s, "}}").unwrap();
        }
    }

    let filename = snake_case(tm.name.0.as_str());
    EmittedFile {
        path: PathBuf::from(format!("src/tests/{filename}.rs")),
        content: s,
    }
}

/// `src/tests/mod.rs` — declares the per-file test modules.
pub(super) fn emit_tests_mod(test_modules: &[TestModule]) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    for tm in test_modules {
        writeln!(s, "pub mod {};", snake_case(tm.name.0.as_str())).unwrap();
    }
    EmittedFile {
        path: PathBuf::from("src/tests/mod.rs"),
        content: s,
    }
}

/// Convert a Ruby test description (`"creates an article with valid
/// attributes"`) to a valid Rust function name. Non-word characters
/// become underscores; leading/trailing underscores stripped.
fn test_fn_name(desc: &str) -> String {
    let mut s: String = desc
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    // Collapse runs of `_`.
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_matches('_').to_string()
}

/// Heuristic: does the test body reference runtime support we haven't
/// built yet? Phase 3 brought SQLite-backed persistence, associations,
/// belongs_to existence, dependent destroy, and assert_difference —
/// all previous skip reasons for real-blog now have emit support.
/// Keep the walk as a safety net for any future test body whose shape
/// exceeds what the current emitter handles; real-blog currently
/// triggers none of the remaining cases.
fn test_needs_runtime_unsupported(test: &Test) -> bool {
    test_body_uses_unsupported(&test.body)
}

fn test_body_uses_unsupported(_e: &Expr) -> bool {
    // Phase 3 rounded out the list of Ruby/Rails primitives the Rust
    // emitter handles; no real-blog pattern currently forces a skip.
    // Add shape-specific checks back here if a future fixture demands.
    false
}
