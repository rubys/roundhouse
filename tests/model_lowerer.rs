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

use roundhouse::dialect::LibraryClass;
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
