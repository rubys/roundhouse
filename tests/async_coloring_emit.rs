//! Phase 3 of async coloring (`project_async_coloring_plan.md`).
//!
//! - **Gate 1 (critical):** the `node-sync` profile produces emit
//!   byte-equal to the implicit-default `emit(app)`. Proves the
//!   coloring path is opt-in and the pre-Phase-3 output is preserved.
//! - **Gate 2 (smoke):** the `node-async` profile produces output
//!   that contains `async ` and `await ` somewhere — minimal proof
//!   the propagation + emit path actually fires under an async
//!   profile. Full Gate 2 (real-blog tests against pg/libsql) lives
//!   in the Phase-4 validation work.
//!
//! Each test runs against multiple fixtures so a regression in one
//! lowering surface (controllers, models, views, tests) doesn't hide
//! a regression in another.

use std::path::Path;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::typescript;
use roundhouse::ingest::ingest_app;
use roundhouse::profile::DeploymentProfile;

fn analyzed(fixture: &str) -> roundhouse::App {
    let mut app = ingest_app(Path::new(fixture)).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn assert_byte_equal(
    fixture: &str,
    a: &[roundhouse::emit::EmittedFile],
    b: &[roundhouse::emit::EmittedFile],
) {
    assert_eq!(
        a.len(),
        b.len(),
        "fixture {fixture}: file count mismatch ({} vs {})",
        a.len(),
        b.len()
    );
    for (fa, fb) in a.iter().zip(b.iter()) {
        assert_eq!(
            fa.path, fb.path,
            "fixture {fixture}: file order mismatch"
        );
        assert_eq!(
            fa.content, fb.content,
            "fixture {fixture}: content mismatch in {}\n--- emit() ---\n{}\n--- emit_with_profile(node_sync) ---\n{}",
            fa.path.display(),
            fa.content,
            fb.content,
        );
    }
}

#[test]
fn gate_1_node_sync_byte_equal_to_emit_tiny_blog() {
    let app = analyzed("fixtures/tiny-blog");
    let baseline = typescript::emit(&app);
    let with_profile = typescript::emit_with_profile(&app, &DeploymentProfile::node_sync());
    assert_byte_equal("tiny-blog", &baseline, &with_profile);
}

#[test]
fn gate_1_node_sync_byte_equal_to_emit_real_blog() {
    let app = analyzed("fixtures/real-blog");
    let baseline = typescript::emit(&app);
    let with_profile = typescript::emit_with_profile(&app, &DeploymentProfile::node_sync());
    assert_byte_equal("real-blog", &baseline, &with_profile);
}

/// Diagnostic: dumps every method marked async by propagation
/// against real-blog under the node-async extern list, plus the
/// first async-matching Send found in each method's body. Useful
/// for tracking down propagation over-marking; the
/// receiver-aware filter (`recv_is_known_sync`) and the
/// parameter-name filter (`body_calls_async_with_params`) were
/// both diagnosed via this dump.
#[test]
#[ignore]
fn dump_propagation_marks_real_blog() {
    use roundhouse::analyze::async_color;
    use roundhouse::expr::{Expr, ExprNode, LValue};
    use roundhouse::profile::DeploymentProfile;
    use std::collections::HashSet;

    let app = analyzed("fixtures/real-blog");
    let profile = DeploymentProfile::node_async();
    let extern_names: Vec<&'static str> = profile.adapter().async_seed_methods().to_vec();

    // Full pipeline reproduction.
    let preliminary_views: Vec<roundhouse::dialect::LibraryClass> = app
        .views
        .iter()
        .map(|v| roundhouse::lower::lower_view_to_library_class(v, &app))
        .collect();
    let view_extras: Vec<(roundhouse::ident::ClassId, roundhouse::analyze::ClassInfo)> =
        preliminary_views
            .iter()
            .map(|c| {
                (
                    c.name.clone(),
                    roundhouse::lower::class_info_from_library_class(c),
                )
            })
            .collect();
    let mut route_helper_funcs = roundhouse::lower::lower_routes_to_library_functions(&app);
    let params_specs_full =
        roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    let params_specs_simple: std::collections::BTreeMap<roundhouse::ident::Symbol, Vec<roundhouse::ident::Symbol>> =
        params_specs_full
            .iter()
            .map(|(r, s)| (r.clone(), s.fields.clone()))
            .collect();
    let (mut model_lcs, model_registry) = roundhouse::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        view_extras,
        &params_specs_simple,
    );

    let mut view_lower_extras: Vec<(roundhouse::ident::ClassId, roundhouse::analyze::ClassInfo)> =
        model_registry.clone().into_iter().collect();
    let mut view_lcs = roundhouse::lower::lower_views_to_library_classes(
        &app.views,
        &app,
        view_lower_extras.clone(),
    );
    let view_extras2: Vec<(roundhouse::ident::ClassId, roundhouse::analyze::ClassInfo)> =
        view_lcs
            .iter()
            .map(|c| (c.name.clone(), roundhouse::lower::class_info_from_library_class(c)))
            .collect();
    let mut controller_extras: Vec<(roundhouse::ident::ClassId, roundhouse::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    controller_extras.extend(view_extras2);
    let mut controller_lcs = roundhouse::lower::lower_controllers_to_library_classes(
        &app.controllers,
        controller_extras,
    );
    let mut fixture_lcs = roundhouse::lower::lower_fixtures_to_library_classes(&app);

    let mut all_classes: Vec<roundhouse::dialect::LibraryClass> = Vec::new();
    let m_len = model_lcs.len();
    let v_len = view_lcs.len();
    let c_len = controller_lcs.len();
    let _fx_len = fixture_lcs.len();
    all_classes.append(&mut model_lcs);
    all_classes.append(&mut view_lcs);
    all_classes.append(&mut controller_lcs);
    all_classes.append(&mut fixture_lcs);

    async_color::propagate_global_with_externs(
        &mut all_classes,
        &mut route_helper_funcs,
        &extern_names,
    );

    let _ = view_lower_extras;
    let _ = (v_len, c_len);

    // Build the after-set of async method names — across ALL classes
    // (model + view + controller + fixture) plus the extern set, plus
    // route_helper_funcs.
    let mut async_names: HashSet<String> = all_classes
        .iter()
        .flat_map(|c| {
            c.methods
                .iter()
                .filter(|m| m.is_async)
                .map(|m| m.name.as_str().to_string())
        })
        .collect();
    async_names.extend(
        route_helper_funcs
            .iter()
            .filter(|f| f.is_async)
            .map(|f| f.name.as_str().to_string()),
    );
    async_names.extend(extern_names.iter().map(|s| s.to_string()));

    println!("=== Class names ===");
    for c in &all_classes {
        println!("  {}", c.name.0);
    }
    // Dump every Send method name in Views::Articles#form for diagnosis.
    let form_class = all_classes.iter().find(|c| {
        c.name.0.as_str().contains("Articles") && c.methods.iter().any(|m| m.name.as_str() == "form")
    });
    if let Some(c) = form_class {
        if let Some(form_method) = c.methods.iter().find(|m| m.name.as_str() == "form") {
            println!("=== Sends in Views::Articles#form body ===");
            let mut sends: Vec<(String, Option<String>)> = Vec::new();
            collect_sends(&form_method.body, &mut sends);
            for (m, recv) in &sends {
                println!("  Send method={m:?} recv={recv:?}");
            }
            fn collect_sends(e: &Expr, out: &mut Vec<(String, Option<String>)>) {
                match &*e.node {
                    ExprNode::Send { recv, method, args, block, .. } => {
                        let recv_desc = recv
                            .as_ref()
                            .map(|r| format!("{:?}", std::mem::discriminant(&*r.node)));
                        out.push((method.as_str().to_string(), recv_desc));
                        if let Some(r) = recv {
                            collect_sends(r, out);
                        }
                        for a in args {
                            collect_sends(a, out);
                        }
                        if let Some(b) = block {
                            collect_sends(b, out);
                        }
                    }
                    ExprNode::Seq { exprs } => exprs.iter().for_each(|x| collect_sends(x, out)),
                    ExprNode::If { cond, then_branch, else_branch } => {
                        collect_sends(cond, out);
                        collect_sends(then_branch, out);
                        collect_sends(else_branch, out);
                    }
                    ExprNode::Assign { value, .. } => collect_sends(value, out),
                    ExprNode::Lambda { body, .. } => collect_sends(body, out),
                    ExprNode::BoolOp { left, right, .. } => {
                        collect_sends(left, out);
                        collect_sends(right, out);
                    }
                    ExprNode::Let { value, body, .. } => {
                        collect_sends(value, out);
                        collect_sends(body, out);
                    }
                    ExprNode::Cast { value, .. } => collect_sends(value, out),
                    ExprNode::Return { value } => collect_sends(value, out),
                    ExprNode::Hash { entries, .. } => {
                        for (k, v) in entries {
                            collect_sends(k, out);
                            collect_sends(v, out);
                        }
                    }
                    ExprNode::Array { elements, .. } => {
                        for x in elements {
                            collect_sends(x, out);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    println!("=== Full-pipeline async marks (real-blog) ===");
    println!("extern: {extern_names:?}");
    let mut sorted_async: Vec<&String> = async_names.iter().collect();
    sorted_async.sort();
    println!("async name set: {sorted_async:?}");
    println!("--- which classes own each async name? ---");
    for name in &sorted_async {
        let owners: Vec<String> = all_classes
            .iter()
            .filter_map(|c| {
                c.methods
                    .iter()
                    .find(|m| m.is_async && m.name.as_str() == name.as_str())
                    .map(|_| c.name.0.as_str().to_string())
            })
            .collect();
        let func_owner = route_helper_funcs
            .iter()
            .any(|f| f.is_async && f.name.as_str() == name.as_str());
        if !owners.is_empty() || func_owner {
            println!("  `{name}` → {owners:?} (func={func_owner})");
        }
    }
    println!("--- models (first {m_len} classes in all_classes) ---");
    for class in all_classes.iter().take(m_len) {
        for m in &class.methods {
            if !m.is_async {
                continue;
            }
            let trigger = first_async_send_name(&m.body, &async_names);
            match trigger {
                Some(t) => println!("  {}#{}  ← Send to `{t}`", class.name.0, m.name),
                None => println!(
                    "  {}#{}  ← NO ASYNC SEND IN BODY (over-mark candidate!)",
                    class.name.0, m.name
                ),
            }
        }
    }
    println!("--- non-models (controllers/views/fixtures) ---");
    for class in all_classes.iter().skip(m_len) {
        for m in &class.methods {
            if !m.is_async {
                continue;
            }
            let trigger = first_async_send_name(&m.body, &async_names);
            match trigger {
                Some(t) => println!("  {}#{}  ← Send to `{t}`", class.name.0, m.name),
                None => println!(
                    "  {}#{}  ← NO ASYNC SEND IN BODY (over-mark candidate!)",
                    class.name.0, m.name
                ),
            }
        }
    }

    fn first_async_send_name(body: &Expr, async_names: &HashSet<String>) -> Option<String> {
        let mut found: Option<String> = None;
        walk(body, async_names, &mut found);
        found
    }
    fn walk(e: &Expr, async_names: &HashSet<String>, found: &mut Option<String>) {
        if found.is_some() {
            return;
        }
        match &*e.node {
            ExprNode::Send { recv, method, args, block, .. } => {
                if async_names.contains(method.as_str()) {
                    *found = Some(method.as_str().to_string());
                    return;
                }
                if let Some(r) = recv {
                    walk(r, async_names, found);
                }
                for a in args {
                    walk(a, async_names, found);
                }
                if let Some(b) = block {
                    walk(b, async_names, found);
                }
            }
            ExprNode::Seq { exprs } => exprs.iter().for_each(|x| walk(x, async_names, found)),
            ExprNode::If { cond, then_branch, else_branch } => {
                walk(cond, async_names, found);
                walk(then_branch, async_names, found);
                walk(else_branch, async_names, found);
            }
            ExprNode::Assign { target, value } => {
                walk_lvalue(target, async_names, found);
                walk(value, async_names, found);
            }
            ExprNode::Lambda { body, .. } => walk(body, async_names, found),
            ExprNode::BoolOp { left, right, .. } => {
                walk(left, async_names, found);
                walk(right, async_names, found);
            }
            ExprNode::Let { value, body, .. } => {
                walk(value, async_names, found);
                walk(body, async_names, found);
            }
            ExprNode::Cast { value, .. } => walk(value, async_names, found),
            ExprNode::Return { value } => walk(value, async_names, found),
            ExprNode::Apply { fun, args, block } => {
                walk(fun, async_names, found);
                for a in args {
                    walk(a, async_names, found);
                }
                if let Some(b) = block {
                    walk(b, async_names, found);
                }
            }
            ExprNode::Array { elements, .. } => {
                for x in elements {
                    walk(x, async_names, found);
                }
            }
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    walk(k, async_names, found);
                    walk(v, async_names, found);
                }
            }
            ExprNode::Case { scrutinee, arms } => {
                walk(scrutinee, async_names, found);
                for a in arms {
                    if let Some(g) = &a.guard {
                        walk(g, async_names, found);
                    }
                    walk(&a.body, async_names, found);
                }
            }
            _ => {}
        }
    }
    fn walk_lvalue(lv: &LValue, async_names: &HashSet<String>, found: &mut Option<String>) {
        match lv {
            LValue::Attr { recv, .. } => walk(recv, async_names, found),
            LValue::Index { recv, index } => {
                walk(recv, async_names, found);
                walk(index, async_names, found);
            }
            _ => {}
        }
    }
}

#[test]
#[ignore]
fn dump_libsql_runtime_real_blog() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::node_async());
    for f in &files {
        let p = f.path.to_string_lossy();
        if p == "package.json" {
            println!("=== {p} (full) ===");
            for line in f.content.lines() {
                println!("  {line}");
            }
        } else if p == "src/juntos.ts" || p == "src/server.ts" {
            println!("=== {p} (first 8 lines) ===");
            for line in f.content.lines().take(8) {
                println!("  {line}");
            }
        }
    }
}

#[test]
#[ignore]
fn dump_async_lines_real_blog() {
    // Run with: cargo test --test async_coloring_emit dump_async_lines_real_blog -- --ignored --nocapture
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::node_async());
    for f in &files {
        let path_s = f.path.display().to_string();
        if path_s.contains("articles_controller")
            || path_s.contains("active_record/base")
            || path_s.contains("articles.ts")
            || path_s.contains("seeds.ts")
            || path_s.contains("route_helpers.ts")
        {
            println!("// === {path_s} ===");
            for (i, line) in f.content.lines().enumerate() {
                if line.contains("async ") || line.contains("await ") || line.contains("function ") {
                    println!("{:4}: {}", i + 1, line);
                }
            }
        }
    }
}

#[test]
fn gate_2_node_async_emits_async_and_await() {
    // Minimal smoke: under the async profile, the controller layer
    // (which calls AR class methods like `Post.all`) should pick up
    // `async ` on action methods and `await ` at AR Send sites. Full
    // semantic verification is in the Phase-4 toolchain tests against
    // a real async DB driver.
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::node_async());
    let combined: String = files
        .iter()
        .map(|f| f.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        combined.contains("async "),
        "expected `async ` somewhere in node-async output",
    );
    assert!(
        combined.contains("await "),
        "expected `await ` somewhere in node-async output",
    );
    // Async return-type wrapping: every async method's return slot
    // must be `Promise<...>`. Without this, tsc rejects the file —
    // an `async` function declared `: void` is a TypeScript error
    // (TS1064: 'async' return type must be a Promise).
    assert!(
        combined.contains("Promise<void>"),
        "expected `Promise<void>` on async no-return methods in node-async output",
    );
    assert!(
        combined.contains("Promise<Article>"),
        "expected `Promise<Article>` on async fixture/finder methods in node-async output",
    );
    // Negative: no async method should declare a bare non-Promise
    // return. Specifically `async <name>(): void` is the regression
    // we want to catch.
    for line in combined.lines() {
        if line.contains("async ") && line.contains("function ") || line.contains("async ") && line.contains("(") {
            assert!(
                !line.contains("): void {") && !line.contains("): void;"),
                "async method declared with bare `void` return (must be `Promise<void>`):\n{line}",
            );
        }
    }
    // LibraryFunction async emission: the seeds runner calls
    // `Article.create!(...)` which is in the AR adapter surface
    // (seed extern), so propagation must mark the runner async.
    // `export async function run(): Promise<void>` is the canonical
    // shape.
    let seeds = files
        .iter()
        .find(|f| f.path.to_str().is_some_and(|p| p.contains("db/seeds.ts")))
        .expect("seeds file should be emitted for real-blog");
    assert!(
        seeds.content.contains("export async function run(): Promise<void>"),
        "seeds runner should be async (calls AR adapter methods); got:\n{}",
        seeds.content,
    );
    // Route helpers don't call AR — stay sync. This guards against
    // over-marking (a regression in the propagation pass).
    let helpers = files
        .iter()
        .find(|f| f.path.to_str().is_some_and(|p| p.contains("route_helpers.ts")));
    if let Some(helpers) = helpers {
        for line in helpers.content.lines() {
            assert!(
                !line.contains("async function "),
                "route helper functions are pure URL builders; they must not be async: {line}",
            );
        }
    }

    // Profile-aware runtime selection: node-async ships the libsql
    // variants of juntos.ts and server.ts (not better-sqlite3).
    let juntos = files
        .iter()
        .find(|f| f.path.to_str() == Some("src/juntos.ts"))
        .expect("src/juntos.ts should be emitted");
    assert!(
        juntos.content.contains("LibsqlActiveRecordAdapter"),
        "node-async juntos.ts should be the libsql variant"
    );
    assert!(
        juntos.content.contains("@libsql/client"),
        "node-async juntos.ts should import @libsql/client"
    );
    // Negative: no actual import of better-sqlite3. (References
    // in comments are fine — the libsql variant references the
    // sqlite variant in context-explaining commentary.)
    for line in juntos.content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("*") || trimmed.starts_with("/*") {
            continue;
        }
        assert!(
            !line.contains("better-sqlite3"),
            "node-async juntos.ts must not import better-sqlite3 in code: {line}"
        );
    }
    let server = files
        .iter()
        .find(|f| f.path.to_str() == Some("src/server.ts"))
        .expect("src/server.ts should be emitted");
    assert!(
        server.content.contains("createClient"),
        "node-async server.ts should use libsql createClient"
    );
    for line in server.content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("*") || trimmed.starts_with("/*") {
            continue;
        }
        assert!(
            !line.contains("better-sqlite3"),
            "node-async server.ts must not import better-sqlite3 in code: {line}"
        );
    }

    // package.json picks the right DB dependency.
    let pkg = files
        .iter()
        .find(|f| f.path.to_str() == Some("package.json"))
        .expect("package.json should be emitted");
    assert!(
        pkg.content.contains("@libsql/client"),
        "node-async package.json should depend on @libsql/client; got:\n{}",
        pkg.content,
    );
    assert!(
        !pkg.content.contains("better-sqlite3"),
        "node-async package.json must not depend on better-sqlite3"
    );
}
