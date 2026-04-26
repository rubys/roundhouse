//! Anchor for step 1 of the spinel-blog plan: ingest a spinel-blog
//! runtime file through the library-shape pipeline, emit Ruby, verify
//! the IR captures the semantics.
//!
//! Note on goal: this is *not* a strict source-equivalence round-trip.
//! `attr_reader :foo` is lowered to `def foo; @foo; end` at ingest
//! time (per the YAGNI-on-round-trip decision); emitted Ruby differs
//! syntactically from input. The forcing function is "Spinel can
//! compile the emitted Ruby and the result behaves the same as the
//! original" — surface preservation is not the goal.
//!
//! Smallest non-trivial entry: `runtime/active_record/errors.rb` —
//! two classes inside `module ActiveRecord` (`RecordNotFound`,
//! `RecordInvalid`), one ivar, one `super` call with a string-interp
//! arg, one `attr_reader` (which lowers to a getter method).

use std::path::PathBuf;

use roundhouse::App;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

const ERRORS_RB_PATH: &str = "fixtures/spinel-blog/runtime/active_record/errors.rb";
const INFLECTOR_RB_PATH: &str = "fixtures/spinel-blog/runtime/inflector.rb";

#[test]
fn errors_rb_ingests_and_emits_via_library_path() {
    let path = PathBuf::from(ERRORS_RB_PATH);
    let source = std::fs::read(&path).expect("read errors.rb");
    let path_str = path.display().to_string();

    let classes = ingest_library_classes(&source, &path_str)
        .expect("ingest_library_classes returned Err");

    // Both classes from errors.rb should land. Names are last-segment
    // (Prism reports the syntactic name; module nesting is implicit).
    assert_eq!(
        classes.len(),
        2,
        "expected RecordNotFound + RecordInvalid; got {} ({:?})",
        classes.len(),
        classes.iter().map(|c| c.name.0.as_str().to_string()).collect::<Vec<_>>(),
    );
    let names: Vec<&str> = classes.iter().map(|c| c.name.0.as_str()).collect();
    assert!(names.contains(&"RecordNotFound"), "names: {names:?}");
    assert!(names.contains(&"RecordInvalid"), "names: {names:?}");

    // Both inherit from StandardError.
    for lc in &classes {
        let parent = lc.parent.as_ref().map(|p| p.0.as_str()).unwrap_or("(none)");
        assert_eq!(parent, "StandardError", "{}: parent {parent}", lc.name.0.as_str());
    }

    // RecordInvalid is the rich one. attr_reader :record lowers to
    // a getter method, so the methods Vec should hold both that
    // synthesized getter and the source-defined initialize.
    let invalid = classes
        .iter()
        .find(|c| c.name.0.as_str() == "RecordInvalid")
        .expect("RecordInvalid present");
    let method_names: Vec<&str> = invalid.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(
        method_names.contains(&"record"),
        "expected synthesized getter for attr_reader :record; got {method_names:?}",
    );
    assert!(
        method_names.contains(&"initialize"),
        "expected initialize method; got {method_names:?}",
    );

    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let files = emit_library(&app);
    assert_eq!(files.len(), 2, "one file per LibraryClass");

    let invalid_file = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("record_invalid.rb"))
        .expect("record_invalid.rb emitted");
    let content = &invalid_file.content;

    // Class shell + parent.
    assert!(content.contains("class RecordInvalid < StandardError"), "emitted: {content}");
    // Lowered attr_reader: a `def record` returning `@record`.
    assert!(content.contains("def record"), "emitted: {content}");
    assert!(content.contains("@record"), "emitted: {content}");
    // The source-defined initialize body round-trips.
    assert!(content.contains("def initialize(record)"), "emitted: {content}");
    assert!(content.contains("super("), "emitted: {content}");
    assert!(content.trim_end().ends_with("end"), "emitted: {content}");
}

/// `runtime/inflector.rb`: a `module Inflector` with one `def
/// self.pluralize`. Lowered to a `LibraryClass` with no parent and
/// the singleton method (per the YAGNI-on-round-trip decision —
/// callers only use `Inflector.pluralize(...)`, never `include`, so
/// module-vs-class distinction can collapse to class semantics).
#[test]
fn inflector_rb_ingests_module_as_namespace() {
    let path = PathBuf::from(INFLECTOR_RB_PATH);
    let source = std::fs::read(&path).expect("read inflector.rb");
    let path_str = path.display().to_string();

    let classes = ingest_library_classes(&source, &path_str)
        .expect("ingest_library_classes returned Err");

    assert_eq!(
        classes.len(),
        1,
        "expected one LibraryClass (Inflector); got {} ({:?})",
        classes.len(),
        classes.iter().map(|c| c.name.0.as_str().to_string()).collect::<Vec<_>>(),
    );

    let inflector = &classes[0];
    assert_eq!(inflector.name.0.as_str(), "Inflector");
    assert!(inflector.parent.is_none(), "module-as-namespace has no parent");
    assert_eq!(inflector.methods.len(), 1);

    let m = &inflector.methods[0];
    assert_eq!(m.name.as_str(), "pluralize");
    // `def self.pluralize` → MethodReceiver::Class.
    assert!(
        matches!(m.receiver, roundhouse::dialect::MethodReceiver::Class),
        "expected class-method receiver; got {:?}",
        m.receiver,
    );
    assert_eq!(
        m.params.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        vec!["count", "word"],
    );

    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let files = emit_library(&app);
    assert_eq!(files.len(), 1);
    let content = &files[0].content;

    // Module lowered to class shell; singleton method emits as `def self.x`.
    assert!(content.contains("class Inflector"), "emitted: {content}");
    assert!(content.contains("def self.pluralize(count, word)"), "emitted: {content}");
    assert!(content.contains("if "), "emitted: {content}");
    assert!(content.trim_end().ends_with("end"), "emitted: {content}");
}
