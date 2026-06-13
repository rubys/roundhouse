//! Span preservation through the model lowerer — the model counterpart
//! of `view_span_preservation.rs` / `controller_span_preservation.rs`.
//! Model bodies are almost entirely synthesis: schema-derived accessors,
//! adapter primitives, association readers, the `validate` body, the
//! broadcasts_to lifecycle expansion. The typed dialect items
//! (Association / Validation / Callback) drop their source `Expr` at
//! ingest, so the declaration span rides the `ModelBodyItem` wrapper and
//! the synthesizers stamp it; whatever has no finer provenance inherits
//! the model's class-declaration span (file-grain catch-all in
//! `build_methods`).

use roundhouse::expr::Expr;
use roundhouse::ingest::ingest_app;

fn for_each_expr(e: &mut Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    e.node.for_each_child_mut(&mut |c| for_each_expr(c, f));
}

/// Bulk-lower real-blog's models the way whole-app emit does (Arel
/// rewrite + body-typing included), with a params spec so the
/// `from_params` factory synthesis is exercised too.
fn lowered_models(app: &roundhouse::App) -> Vec<roundhouse::dialect::LibraryClass> {
    let mut params_specs = std::collections::BTreeMap::new();
    params_specs.insert(
        roundhouse::Symbol::from("article"),
        vec![roundhouse::Symbol::from("title"), roundhouse::Symbol::from("body")],
    );
    roundhouse::lower::lower_models_to_library_classes_with_params(
        &app.models,
        &app.schema,
        Vec::new(),
        &params_specs,
    )
}

#[test]
fn lowered_model_bodies_carry_no_synthetic_spans() {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    let lcs = lowered_models(&app);
    assert!(!lcs.is_empty(), "fixture should have models");
    let mut saw_row_class = false;
    for lc in &lcs {
        let is_row = lc.name.0.as_str().ends_with("Row");
        saw_row_class |= is_row;
        // Row classes have no source file of their own — they attribute
        // to the owning model's file.
        let owner = lc.name.0.as_str().trim_end_matches("Row");
        let expected_file =
            format!("app/models/{}.rb", roundhouse::naming::snake_case(owner));
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
            // Everything in a lowered model body comes from the model's
            // own source file.
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
                source.path.ends_with(&expected_file),
                "{} method {}: spans resolve to {}, expected {}",
                lc.name.0.as_str(),
                m.name.as_str(),
                source.path,
                expected_file,
            );
        }
    }
    assert!(saw_row_class, "bulk lowering should synthesize Row classes");
}

/// Declaration-grain, not just file-grain: each synthesized method
/// attributes to the DSL line it came from, not the whole class.
#[test]
fn synthesized_methods_attribute_to_their_declarations() {
    let app = ingest_app(std::path::Path::new("fixtures/real-blog")).expect("ingest real-blog");
    let lcs = lowered_models(&app);
    let article = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "Article")
        .expect("Article LibraryClass");
    let source = {
        let comments = article
            .methods
            .iter()
            .find(|m| m.name.as_str() == "comments")
            .expect("comments reader");
        let file = comments.body.span.file;
        assert_ne!(file.0, 0, "comments reader body should carry a real span");
        &app.sources[file.0 as usize - 1]
    };
    assert!(source.path.ends_with("app/models/article.rb"), "{}", source.path);

    // has_many reader lands exactly on its declaration.
    let comments = article
        .methods
        .iter()
        .find(|m| m.name.as_str() == "comments")
        .unwrap();
    let has_many_at = source.text.find("has_many :comments").unwrap() as u32;
    assert_eq!(
        comments.body.span.start, has_many_at,
        "comments reader should attribute to the has_many line",
    );

    // validate body: title and body checks land on their own
    // `validates` lines (statement grain within one method).
    let validate = article
        .methods
        .iter()
        .find(|m| m.name.as_str() == "validate")
        .expect("validate method");
    let stmts = match &*validate.body.node {
        roundhouse::expr::ExprNode::Seq { exprs } => exprs,
        other => panic!("expected Seq validate body, got {other:?}"),
    };
    let title_at = source.text.find("validates :title").unwrap() as u32;
    let body_at = source.text.find("validates :body").unwrap() as u32;
    let starts: Vec<u32> = stmts.iter().map(|e| e.span.start).collect();
    assert!(
        starts.contains(&title_at),
        "validate stmts should include the `validates :title` offset; got {starts:?}",
    );
    assert!(
        starts.contains(&body_at),
        "validate stmts should include the `validates :body` offset; got {starts:?}",
    );

    // broadcasts_to expansion attributes to the broadcasts_to line.
    let after_create = article
        .methods
        .iter()
        .find(|m| m.name.as_str() == "after_create_commit")
        .expect("after_create_commit (broadcasts_to expansion)");
    let broadcasts_at = source.text.find("broadcasts_to").unwrap() as u32;
    let mut body = after_create.body.clone();
    let mut hit = false;
    for_each_expr(&mut body, &mut |e| {
        hit |= e.span.start == broadcasts_at;
    });
    assert!(hit, "after_create_commit should carry the broadcasts_to declaration span");

    // Whole-cloth schema accessors fall back to the class declaration
    // (file-grain catch-all), still inside article.rb.
    let title_reader = article
        .methods
        .iter()
        .find(|m| m.name.as_str() == "title")
        .expect("title accessor");
    assert_eq!(title_reader.body.span.file, comments.body.span.file);
}
