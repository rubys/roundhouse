//! A block's REST parameter survives to the emit
//! (`ingest::expr::block_rest_param` → `ExprNode::Lambda::rest_param`).
//!
//! `block_param_names` collected `requireds()` and nothing else, so
//! `|*args|` reached the IR as a block with NO parameters. That is not
//! a degradation but a CORRUPTION: the body still reads the name, and
//! an emitted module answers a bare name from its own functions when no
//! local binds it. campfire's Opengraph tests stub a socket with
//!
//! ```text
//! TCPSocket.expects(:open).with { |*args, **| args.first == @url.host }
//! ```
//!
//! and unparameterized that block died on `undefined local variable or
//! method 'args'` — from inside the mocha matcher, naming nothing about
//! the block.
//!
//! THE FIELD HAS TO BE THREADED, not just collected. Every pass that
//! rebuilds a Lambda — and there are a dozen — resets whatever it does
//! not carry, so a field added and then dropped one rewrite later is
//! invisible in exactly the same way. That is why this asserts on the
//! EMIT and not on the ingest.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(test_body: &str) -> String {
    let src = format!(
        "require \"test_helper\"\n\nclass PostTest < ActiveSupport::TestCase\n  test \"one\" do\n    {test_body}\n  end\nend\n"
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
        .expect("post_test emitted")
        .content
        .clone()
}

/// campfire's shape, verbatim: a rest plus an ANONYMOUS double-splat.
#[test]
fn a_rest_parameter_reaches_the_emitted_block() {
    let src = emitted("Post.all.each { |*args, **| args.first }");
    assert!(
        src.contains("|*args|"),
        "the splat is a parameter, not a dropped one:\n{src}"
    );
}

/// A named rest ALONGSIDE required params keeps both, in order.
#[test]
fn a_rest_after_requireds_keeps_both() {
    let src = emitted("Post.all.each { |first, *rest| first }");
    assert!(src.contains("|first, *rest|"), "both, in order:\n{src}");
}

/// An ANONYMOUS rest (`|*|`) binds nothing and no body can read it, so
/// it stays absent rather than inventing a name.
#[test]
fn an_anonymous_rest_stays_absent() {
    let src = emitted("Post.all.each { |*| Post.count }");
    assert!(!src.contains('*'), "nothing to bind, nothing emitted:\n{src}");
}

/// A block with no rest is byte-identical to what it always was.
#[test]
fn a_plain_block_is_unchanged() {
    let src = emitted("Post.all.each { |p| p.body }");
    assert!(src.contains("|p|"), "{src}");
    assert!(!src.contains("|*"), "no splat appears from nowhere:\n{src}");
}
