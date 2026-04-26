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
use roundhouse::ingest::ingest_library_class;

const ERRORS_RB_PATH: &str = "fixtures/spinel-blog/runtime/active_record/errors.rb";

#[test]
#[ignore = "library-shape gap inventory; see module docstring"]
fn errors_rb_round_trips_via_library_path() {
    let path = PathBuf::from(ERRORS_RB_PATH);
    let source = std::fs::read(&path).expect("read errors.rb");
    let path_str = path.display().to_string();

    // Gap 1+2: today ingest_library_class returns Ok(None) because
    // find_first_class doesn't descend into modules. The expect below
    // is what surfaces the gap.
    let lc = ingest_library_class(&source, &path_str)
        .expect("ingest_library_class returned Err")
        .expect("ingest_library_class returned None — module descent missing");

    // Once gap 1 closes, gap 2 surfaces here: only one of the two
    // classes will land. Track both class names so the assertion
    // accepts either resolution choice (FQN or last-segment).
    let name = lc.name.0.as_str();
    assert!(
        name == "RecordNotFound"
            || name == "RecordInvalid"
            || name == "ActiveRecord::RecordNotFound"
            || name == "ActiveRecord::RecordInvalid",
        "unexpected library class name: {name}",
    );

    let mut app = App::new();
    app.library_classes.push(lc);
    let files = emit_library(&app);

    assert_eq!(files.len(), 1, "one file emitted per LibraryClass");
    let content = &files[0].content;

    // Structural assertions — the file should at minimum declare the
    // class with its parent and end with `end`.
    assert!(content.contains("class "), "emitted: {content}");
    assert!(content.contains("< StandardError"), "emitted: {content}");
    assert!(content.trim_end().ends_with("end"), "emitted: {content}");
}
