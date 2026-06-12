//! Span preservation through the view lowerer. The lowerer is almost
//! pure synthesis (the `io` accumulator, `ViewHelpers.*` sends, inlined
//! form HTML, coalesced StringInterps) — these tests pin the convention
//! that every synthesized node inherits the nearest enclosing source
//! span, so emit-time diagnostics on lowered view IR attribute back to
//! a real `file:line:col` instead of rendering location-less.
//!
//! See `walk_body`'s choke-point stamping + `build_library_class`'s
//! file-grain catch-all in `src/lower/view_to_library/`.

use roundhouse::expr::Expr;
use roundhouse::ingest::ingest_app;
use roundhouse::lower::{lower_jbuilder_to_library_class, lower_view_to_library_class};

fn for_each_expr(e: &mut Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    e.node.for_each_child_mut(&mut |c| for_each_expr(c, f));
}

#[test]
fn lowered_view_bodies_carry_no_synthetic_spans() {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    assert!(!app.views.is_empty(), "fixture should have views");
    let mut saw_json = false;
    for view in &app.views {
        // Route by format the same way the bulk emit paths do: ERB
        // (html) through view_to_library, jbuilder (json) through
        // jbuilder_to_library. Both must uphold the same convention.
        let lc = match view.format.as_str() {
            "html" => lower_view_to_library_class(view, &app),
            "json" => {
                saw_json = true;
                lower_jbuilder_to_library_class(view, &app)
            }
            _ => continue,
        };
        for m in &lc.methods {
            let mut body = m.body.clone();
            let mut synthetic: Vec<String> = Vec::new();
            let mut files: std::collections::BTreeSet<u32> =
                std::collections::BTreeSet::new();
            for_each_expr(&mut body, &mut |e| {
                if e.span.is_synthetic() {
                    synthetic.push(format!("{}", e.node.kind_str()));
                } else {
                    files.insert(e.span.file.0);
                }
            });
            assert!(
                synthetic.is_empty(),
                "view {} method {}: {} synthetic-span nodes: {:?}",
                view.name.as_str(),
                m.name.as_str(),
                synthetic.len(),
                synthetic,
            );
            // Every span must point into the one source file this
            // template was ingested from — nothing in a lowered view
            // body comes from anywhere else.
            assert_eq!(
                files.len(),
                1,
                "view {} method {}: expected a single source file, got {files:?}",
                view.name.as_str(),
                m.name.as_str(),
            );
            let file = *files.iter().next().unwrap();
            let source = app
                .sources
                .get(file as usize - 1)
                .unwrap_or_else(|| panic!("FileId {file:?} out of range"));
            assert!(
                (source.path.ends_with(".erb") || source.path.ends_with(".jbuilder"))
                    && source.path.contains(view.name.as_str()),
                "view {}: spans resolve to {}, expected its own template",
                view.name.as_str(),
                source.path,
            );
        }
    }
    assert!(saw_json, "fixture should exercise the jbuilder lowerer");
}

/// Statement-grain, not just file-grain: the top-level statements of a
/// multi-statement template must land on several distinct source
/// offsets. Guards against a regression to "stamp everything with the
/// whole-file span" — file attribution would still pass above, but
/// line/col precision (and the future source-map emit) would be gone.
#[test]
fn lowered_index_view_spans_are_statement_grain() {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "articles/index" && v.format.as_str() == "html")
        .expect("articles/index view");
    let lc = lower_view_to_library_class(view, &app);
    let m = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "index")
        .expect("index method");
    let starts: std::collections::BTreeSet<u32> = match &*m.body.node {
        roundhouse::expr::ExprNode::Seq { exprs } => {
            exprs.iter().map(|e| e.span.start).collect()
        }
        _ => panic!("expected Seq body"),
    };
    assert!(
        starts.len() >= 3,
        "expected >=3 distinct statement start offsets, got {starts:?}",
    );
}
