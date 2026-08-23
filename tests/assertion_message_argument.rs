//! Minitest's trailing failure-message argument, and the assertions it
//! took out of the lowering.
//!
//! `assert test, msg = nil` — every assertion accepts an optional
//! message as its LAST argument. `inline_assertions` guarded the
//! one-argument family on `args.len() == 1`, so the two-argument
//! spelling never matched and passed through as an ordinary Send. No
//! target defines `assert`, so those tests died with `undefined method
//! 'assert'` rather than asserting anything.
//!
//! Measured on campfire, which writes it five times:
//!
//! ```text
//! assert outsiders.any?, "need someone outside the room for this test to mean anything"
//! assert rooms(:hq).reload.name, "HQ"
//! ```
//!
//! The two-argument assertions (`assert_equal` and friends) have always
//! guarded on `>= 2` for exactly this reason. This is the same rule for
//! the one-argument family, plus `assert_not_includes` — Rails' spelling
//! of `refute_includes`, which was in neither table.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

/// Emit one test file whose single test body is `body`.
fn emitted(body: &str) -> String {
    let src = format!(
        "require \"test_helper\"\n\nclass PostTest < ActiveSupport::TestCase\n  test \"one\" do\n    {body}\n  end\nend\n"
    );
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/post.rb"),
            b"class Post < ApplicationRecord\nend\n".to_vec(),
        ),
        (PathBuf::from("test/models/post_test.rb"), src.into_bytes()),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with("post_test.rb"))
        .expect("no post_test.rb emitted")
        .content
        .clone()
}

/// The lowered form is a `raise`; an unlowered one is a bare call to a
/// method that does not exist.
fn assert_lowered(body: &str) {
    let src = emitted(body);
    assert!(src.contains("raise"), "not lowered: {body}\n{src}");
    // Looking for the NAME is not enough — the inlined raise carries it
    // in its own message ("assert_nil failed"). A surviving dispatch is
    // a STATEMENT starting with the name.
    let call = body.split(&[' ', '('][..]).next().unwrap();
    let dispatched = src.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(&format!("{call} ")) || t.starts_with(&format!("{call}("))
    });
    assert!(!dispatched, "left a dispatched `{call}` behind: {src}");
}

#[test]
fn one_arg_assertions_accept_the_trailing_message() {
    assert_lowered("assert Post.count > 0, \"needs a post\"");
    assert_lowered("assert_not Post.count > 5, \"too many\"");
    assert_lowered("refute Post.count > 5, \"too many\"");
    assert_lowered("assert_nil Post.first, \"expected none\"");
    assert_lowered("assert_not_nil Post.first, \"expected one\"");
    assert_lowered("refute_nil Post.first, \"expected one\"");
    assert_lowered("assert_empty Post.all, \"expected none\"");
    assert_lowered("assert_not_empty Post.all, \"expected some\"");
    assert_lowered("refute_empty Post.all, \"expected some\"");
}

/// The message is dropped rather than evaluated — the inlined raise
/// carries its own text.
#[test]
fn the_message_argument_is_dropped() {
    let src = emitted("assert Post.count > 0, \"needs a post\"");
    assert!(!src.contains("needs a post"), "{src}");
    assert!(src.contains("assertion failed"), "{src}");
}

/// Unchanged: the message is optional, so the bare form still lowers.
#[test]
fn the_bare_one_arg_form_still_lowers() {
    assert_lowered("assert Post.count > 0");
    assert_lowered("assert_nil Post.first");
}

/// Rails spells `refute_includes` as `assert_not_includes`, and campfire
/// uses that spelling.
#[test]
fn assert_not_includes_is_refute_includes() {
    let src = emitted("assert_not_includes Post.all, Post.first");
    assert!(src.contains("raise"), "{src}");
    assert!(src.contains("include?"), "{src}");
    assert!(!src.contains("assert_not_includes"), "{src}");
}

/// `assert_throws(:tag) { … }` inlines to a `catch` whose fall-through
/// path clears a flag the throw skips.
///
/// The flag is the whole trick: `catch` answers the block's own last
/// value when nothing throws, and `nil` is a value a `throw` can carry,
/// so the returned value alone cannot tell "threw nil" from "never
/// threw". Without the assertion at all, campfire's Opengraph fetch
/// tests — which prove a resolved IP and never a hostname is what gets
/// connected to, by making the mocked socket throw — died on `undefined
/// method 'assert_throws'`.
#[test]
fn assert_throws_inlines_to_a_catch_with_a_flag() {
    let src = emitted("assert_throws :done do Post.count end");
    assert!(src.contains("catch(:done)"), "lowers to a catch:\n{src}");
    assert!(
        src.contains("__thrown = true") && src.contains("__thrown = false"),
        "the flag is set before the block and cleared on fall-through:\n{src}"
    );
    assert!(
        src.contains("assert_throws failed"),
        "a block that never throws raises:\n{src}"
    );
    assert!(
        !src.lines().any(|l| l.trim_start().starts_with("assert_throws ")),
        "no dispatched assert_throws survives:\n{src}"
    );
}
