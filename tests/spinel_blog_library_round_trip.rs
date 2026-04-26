//! Round-trip the spinel-blog framework runtime through the library-shape
//! pipeline. Forcing function for step 1 of the spinel-blog plan: each
//! `#[ignore]`'d gap surfaced here becomes its own follow-up commit.
//!
//! Smallest non-trivial entry: `runtime/active_record/errors.rb` — two
//! classes inside `module ActiveRecord`, one ivar, one `super`, one
//! `attr_reader`. Exercises the gap inventory directly.
//!
//! Known gaps (close one per commit; un-`#[ignore]` when green):
//!
//! 1. **Module descent.** `src/ingest/util.rs::find_first_class` recurses
//!    through `program_node` and `statements_node` but not through
//!    `module_node`. errors.rb wraps both classes in `module ActiveRecord`,
//!    so the current call returns `None`.
//! 2. **Multiple classes per file.** `ingest_library_class` returns at most
//!    one `LibraryClass`. errors.rb defines two (`RecordNotFound`,
//!    `RecordInvalid`). Need a plural variant or a walker that emits one
//!    `LibraryClass` per class node found.
//! 3. **`attr_reader` / `attr_accessor`.** Explicitly deferred in
//!    `src/ingest/library_class.rs:66-69`. errors.rb uses `attr_reader
//!    :record`. The dialect's `LibraryClass` doesn't carry these today;
//!    they need a new field (or to lower into get/set method pairs at
//!    ingest time).
//! 4. **`super` call.** `RecordInvalid#initialize` calls `super(...)`.
//!    Verify `ExprNode::Super` ingest + ruby emit round-trips cleanly.

use std::path::PathBuf;

use roundhouse::App;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

const ERRORS_RB_PATH: &str = "fixtures/spinel-blog/runtime/active_record/errors.rb";

#[test]
#[ignore = "library-shape gap inventory; see module docstring"]
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

    // RecordInvalid is the rich one. It should carry:
    //  - `attr_reader :record`     (gap 3 — ingest doesn't capture it today)
    //  - `def initialize(record)`  with `super(...)` and ivar assign (gap 4)
    let invalid = classes
        .iter()
        .find(|c| c.name.0.as_str() == "RecordInvalid")
        .expect("RecordInvalid present");
    assert_eq!(
        invalid.methods.len(),
        1,
        "RecordInvalid should have one ingested method (initialize); got {}",
        invalid.methods.len(),
    );
    assert_eq!(invalid.methods[0].name.as_str(), "initialize");

    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let files = emit_library(&app);
    assert_eq!(files.len(), 2, "one file per LibraryClass");

    // Find the rich one in the emitted files.
    let invalid_file = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("record_invalid.rb"))
        .expect("record_invalid.rb emitted");
    let content = &invalid_file.content;

    assert!(content.contains("class RecordInvalid < StandardError"), "emitted: {content}");
    assert!(content.contains("def initialize(record)"), "emitted: {content}");
    // Gap 3: attr_reader :record must round-trip.
    assert!(content.contains("attr_reader :record"), "emitted: {content}");
    // Gap 4: super(...) must round-trip.
    assert!(content.contains("super("), "emitted: {content}");
    assert!(content.trim_end().ends_with("end"), "emitted: {content}");
}
