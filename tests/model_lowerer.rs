//! Step 3 — first session: model lowerers from Rails-shape `Model` to
//! the universal post-lowering `LibraryClass` whose body is a flat
//! sequence of `MethodDef`s. The forcing function is the spinel-blog
//! fixture pair: real-blog/app/models/article.rb (Rails DSL) lowers to
//! a LibraryClass structurally matching spinel-blog/app/models/article.rb
//! (explicit method bodies).
//!
//! Comparison is structural at the IR level — method names, parameter
//! lists, receiver kinds. Body shapes are spot-checked rather than
//! deep-compared because the spinel-blog fixture is hand-written and
//! carries stylistic choices (variable naming, formatting) that the
//! lowerer's output won't match byte-for-byte. See the handoff for the
//! "structural compare passes ≠ textual match required" calibration.

use std::path::Path;

use roundhouse::dialect::{LibraryClass, MethodReceiver};
use roundhouse::ingest::ingest_app;
use roundhouse::lower::lower_model_to_library_class;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/real-blog")
}

fn lower(name: &str) -> LibraryClass {
    let app = ingest_app(fixture_path()).expect("ingest real-blog");
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == name)
        .unwrap_or_else(|| panic!("model {name} not in real-blog"));
    lower_model_to_library_class(model, &app.schema)
}

fn method_names(lc: &LibraryClass) -> Vec<&str> {
    lc.methods.iter().map(|m| m.name.as_str()).collect()
}

#[test]
fn application_record_lowers_to_empty_library_class() {
    // application_record.rb is abstract — no schema table, no
    // associations, no validations. Lowering should produce a
    // LibraryClass with the right name + parent + zero methods.
    let lc = lower("ApplicationRecord");
    assert_eq!(lc.name.0.as_str(), "ApplicationRecord");
    let parent = lc.parent.as_ref().map(|p| p.0.as_str()).unwrap_or("(none)");
    assert_eq!(parent, "ActiveRecord::Base", "parent: {parent}");
    assert!(!lc.is_module);
    assert!(
        lc.methods.is_empty(),
        "ApplicationRecord lowering should have no methods (got {:?})",
        method_names(&lc),
    );
}

#[test]
fn article_lowers_with_schema_methods() {
    let lc = lower("Article");
    assert_eq!(lc.name.0.as_str(), "Article");
    let parent = lc.parent.as_ref().map(|p| p.0.as_str()).unwrap_or("(none)");
    assert_eq!(parent, "ApplicationRecord");

    let names = method_names(&lc);

    // Per-column accessors (excluding id — inherits from base).
    for col in ["title", "body", "created_at", "updated_at"] {
        assert!(names.contains(&col), "missing reader `{col}`: {names:?}");
        let writer = format!("{col}=");
        assert!(
            names.iter().any(|n| *n == writer.as_str()),
            "missing writer `{writer}`: {names:?}",
        );
    }
    // No id reader / writer (id comes from ActiveRecord::Base).
    assert!(
        !names.contains(&"id"),
        "id reader should not be synthesized; methods: {names:?}",
    );

    // The non-attr scaffold: table_name, schema_columns, instantiate,
    // initialize, attributes, [], []=, update.
    for expected in [
        "table_name",
        "schema_columns",
        "instantiate",
        "initialize",
        "attributes",
        "[]",
        "[]=",
        "update",
    ] {
        assert!(
            names.contains(&expected),
            "missing scaffold method `{expected}`: {names:?}",
        );
    }

    // Receiver checks: table_name, schema_columns, instantiate are class
    // methods; everything else is instance.
    let class_methods = ["table_name", "schema_columns", "instantiate"];
    for m in &lc.methods {
        let n = m.name.as_str();
        if class_methods.contains(&n) {
            assert!(
                matches!(m.receiver, MethodReceiver::Class),
                "`{n}` should be a class method, got {:?}",
                m.receiver,
            );
        } else {
            assert!(
                matches!(m.receiver, MethodReceiver::Instance),
                "`{n}` should be an instance method, got {:?}",
                m.receiver,
            );
        }
    }
}
