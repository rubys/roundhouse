//! The autorun shim's per-test guard — what it covers, and what it
//! catches.
//!
//! Both holes here were found by wiring Mocha into campfire's suite,
//! and both cost a whole FILE rather than a test:
//!
//!   * `__t.teardown` ran OUTSIDE the guard, one line below the comment
//!     explaining that anything outside it "would abort the file and
//!     hide every test behind it — the exact failure mode this shim
//!     exists to end". `mocha_verify` raises there on an unmet
//!     `expects`, and one of those took `membership_test` from 8 of 9
//!     to 0 of 9.
//!
//!   * the guard rescued StandardError, and `Mocha::ExpectationError <
//!     Exception` — deliberately, so a test's own `rescue => e` cannot
//!     swallow an unmet expectation. The shim was not swallowing it, it
//!     was MISSING it. Minitest's own runner rescues Exception here for
//!     the same reason.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn shim() -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/post.rb"),
            b"class Post < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            PathBuf::from("test/models/post_test.rb"),
            b"require \"test_helper\"\n\nclass PostTest < ActiveSupport::TestCase\n  test \"one\" do\n    assert true\n  end\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // `emit_spinel` is the entry that carries the autorun shim — the
    // ruby and spinel trees share the emitted test files.
    let files = ruby::emit_spinel(&app);
    files
        .iter()
        .find(|f| f.path.ends_with("post_test.rb"))
        .unwrap_or_else(|| {
            let paths: Vec<_> = files.iter().map(|f| f.path.clone()).collect();
            panic!("no post_test.rb in {paths:?}")
        })
        .content
        .clone()
}

#[test]
fn teardown_runs_inside_a_guard_of_its_own() {
    let src = shim();
    let after_rescue = src
        .split("__t.teardown")
        .next()
        .expect("no teardown in the shim");
    // The teardown call is preceded by its own `begin`, not left bare
    // at the top level of the file.
    assert!(
        after_rescue.trim_end().ends_with("begin"),
        "teardown must be guarded:\n{src}",
    );
}

/// It still has to RUN after a failing test — a stub installed on a
/// real object must not outlive the test that made it — so it lives in
/// a second guard rather than moving above the first rescue.
#[test]
fn teardown_is_not_folded_into_the_test_guard() {
    let src = shim();
    let test_guard = src.split("rescue Exception => __e").next().unwrap_or("");
    assert!(
        !test_guard.contains("__t.teardown"),
        "teardown must not sit inside the test's own guard:\n{src}",
    );
}

#[test]
fn the_guard_catches_exception_not_just_standard_error() {
    let src = shim();
    assert!(src.contains("rescue Exception => __e"), "{src}");
    assert!(
        !src.contains("rescue => __e"),
        "a bare rescue misses Mocha::ExpectationError:\n{src}",
    );
}

/// A test that already failed must not be counted twice when its
/// teardown fails too — `__failed` is the tally's numerator.
#[test]
fn a_failing_teardown_after_a_failing_test_counts_once() {
    let src = shim();
    assert!(src.contains("__ok = true"), "{src}");
    assert!(src.contains("__ok = false"), "{src}");
    assert!(src.contains("if __ok"), "{src}");
}
