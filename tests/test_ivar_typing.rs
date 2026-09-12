//! A test's ivars are typed from its own (setup-inlined) body, so a
//! type-directed rewrite over them fires.
//!
//! `lower_test_modules_to_library_classes` typed every test method
//! with an EMPTY ivar environment: `@messages = ….to_a` bound nothing,
//! and `@messages.third` two lines later was a read off an untyped
//! ivar — which `lower::array_ordinal` (whose rewrite inlines `Array`'s
//! own definition, so it fires only on a receiver the type says IS an
//! Array) correctly declined. campfire's messages_controller_test pages
//! that way twice, and both reached spinel as `undefined method 'id'
//! for unknown`.
//!
//! The setup is inlined ahead of every test, so each body carries its
//! own `@x = …`: one typed pass harvests them, a second pass types the
//! body with them, and the ordinal rewrite runs there — after typing,
//! not in the pre-typing walk over `app.test_modules`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(setup: &str, body: &str) -> String {
    let src = format!(
        "require \"test_helper\"\n\nclass PostTest < ActiveSupport::TestCase\n  setup do\n    {setup}\n  end\n\n  test \"one\" do\n    {body}\n  end\nend\n"
    );
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n    t.datetime \"created_at\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/post.rb"),
            b"class Post < ApplicationRecord\n  scope :ordered, -> { order(:created_at) }\nend\n".to_vec(),
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

#[test]
fn an_ordinal_on_an_ivar_bound_in_setup_is_an_index_read() {
    let src = emitted("@posts = Post.ordered.to_a", "assert_equal @posts.third, @posts.fourth");
    assert!(
        src.contains("@posts[2]") && src.contains("@posts[3]"),
        "the ivar's Array type must reach the ordinal rewrite:\n{src}"
    );
    assert!(!src.contains(".third"), "no `third` should survive:\n{src}");
}

#[test]
fn an_ordinal_on_an_untyped_ivar_is_left_alone() {
    // The rewrite inlines `Array`'s definition, so an ivar the body
    // cannot type keeps the call — the same rule as a local.
    let src = emitted("@thing = mystery", "assert_equal @thing.third, 1");
    assert!(src.contains(".third"), "an untyped ivar must not be rewritten:\n{src}");
}
