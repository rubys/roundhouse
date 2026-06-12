//! Span preservation through the controller lowerer — the controller
//! counterpart of `view_span_preservation.rs`. Action bodies arrive
//! with real spans and the rewrite passes thread them, but three sites
//! synthesize whole-cloth and must inherit enclosing source spans:
//! the `process_action` dispatcher, the `<Resource>Params` classes
//! (provenance = the `permit`/`expect` call), and the Arel pass's
//! SELECT/hydrate expansions (provenance = the chain call site).

use roundhouse::dialect::{LibraryClass, LibraryClassOrigin};
use roundhouse::expr::Expr;
use roundhouse::ingest::ingest_app;

fn for_each_expr(e: &mut Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    e.node.for_each_child_mut(&mut |c| for_each_expr(c, f));
}

/// Mirror of `emit/ruby.rs::lower_controllers_for_spinel` — the bulk
/// path the real emit uses, including the Arel pass (schema + model
/// registry + association graph), so its synthesized expansions are
/// exercised too.
fn lowered_controllers(app: &roundhouse::App) -> Vec<LibraryClass> {
    let (_, model_registry) = roundhouse::lower::lower_models_with_registry(
        &app.models,
        &app.schema,
        Vec::new(),
    );
    let assocs = roundhouse::lower::model_associations::compute_association_graph(app);
    roundhouse::lower::lower_controllers_with_arel_views_and_assocs(
        &app.controllers,
        model_registry.into_iter().collect(),
        Some(&app.schema),
        &app.views,
        &assocs,
    )
}

#[test]
fn lowered_controller_bodies_carry_no_synthetic_spans() {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    let lcs = lowered_controllers(&app);
    assert!(!lcs.is_empty(), "fixture should have controllers");
    let mut saw_params_class = false;
    for lc in &lcs {
        saw_params_class |= matches!(lc.origin, Some(LibraryClassOrigin::ResourceParams { .. }));
        for m in &lc.methods {
            let mut body = m.body.clone();
            let mut synthetic: Vec<String> = Vec::new();
            let mut files: std::collections::BTreeSet<u32> =
                std::collections::BTreeSet::new();
            for_each_expr(&mut body, &mut |e| {
                if e.span.is_synthetic() {
                    synthetic.push(e.node.kind_str().to_string());
                } else {
                    files.insert(e.span.file.0);
                }
            });
            assert!(
                synthetic.is_empty(),
                "{} method {}: {} synthetic-span nodes: {:?}",
                lc.name.0.as_str(),
                m.name.as_str(),
                synthetic.len(),
                synthetic,
            );
            // Everything in a lowered controller method body — including
            // inlined filter statements, Views render rewrites, Arel
            // expansions, and the Params-class bodies recognized from
            // permit calls — derives from controller source, so all
            // spans resolve into one controller file.
            assert_eq!(
                files.len(),
                1,
                "{} method {}: expected a single source file, got {files:?}",
                lc.name.0.as_str(),
                m.name.as_str(),
            );
            let file = *files.iter().next().unwrap();
            let source = app
                .sources
                .get(file as usize - 1)
                .unwrap_or_else(|| panic!("FileId {file} out of range"));
            assert!(
                source.path.contains("app/controllers/")
                    && source.path.ends_with("_controller.rb"),
                "{} method {}: spans resolve to {}, expected a controller file",
                lc.name.0.as_str(),
                m.name.as_str(),
                source.path,
            );
        }
    }
    assert!(
        saw_params_class,
        "expected at least one synthesized <Resource>Params class",
    );
}
