//! Round-trip the spinel-blog framework runtime through the library-shape
//! pipeline. Anchors step 1 of the spinel-blog plan: ingest a runtime
//! file, emit Ruby, verify the rich shapes survive.
//!
//! Smallest non-trivial entry: `runtime/active_record/errors.rb` — two
//! classes inside `module ActiveRecord` (`RecordNotFound`,
//! `RecordInvalid`), one ivar, one `super` call with a string-interp
//! arg, one `attr_reader`. Exercises the library-shape pipeline end
//! to end.
//!
//! When this test starts failing, the delta is the work queue: either
//! a new shape surfaced in spinel-blog that the ingest/emit path
//! doesn't yet handle, or an existing pattern stopped round-tripping.
//! Hold the line.

use std::path::PathBuf;

use roundhouse::App;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

const ERRORS_RB_PATH: &str = "fixtures/spinel-blog/runtime/active_record/errors.rb";

#[test]
fn errors_rb_round_trips_via_library_path() {
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

    // RecordInvalid is the rich one. It should carry the attr_reader
    // and the initialize method.
    let invalid = classes
        .iter()
        .find(|c| c.name.0.as_str() == "RecordInvalid")
        .expect("RecordInvalid present");
    assert_eq!(
        invalid.attrs.len(),
        1,
        "RecordInvalid should have one attr declaration (attr_reader :record)",
    );
    assert_eq!(invalid.attrs[0].names.len(), 1);
    assert_eq!(invalid.attrs[0].names[0].as_str(), "record");
    assert_eq!(invalid.methods.len(), 1, "RecordInvalid: initialize only");
    assert_eq!(invalid.methods[0].name.as_str(), "initialize");

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

    assert!(content.contains("class RecordInvalid < StandardError"), "emitted: {content}");
    assert!(content.contains("attr_reader :record"), "emitted: {content}");
    assert!(content.contains("def initialize(record)"), "emitted: {content}");
    assert!(content.contains("super("), "emitted: {content}");
    assert!(content.trim_end().ends_with("end"), "emitted: {content}");
}
