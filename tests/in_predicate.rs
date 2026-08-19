//! ActiveSupport's `Object#in?` → `<arg>.include?(<recv>)`.
//!
//! The gap this closes is the one a type table hides: `analyze::body::
//! send` has answered `Ty::Bool` for `in?` since before anything
//! implemented it, so nothing looked missing until a body ran. All ten
//! tests in campfire's `opengraph_metadata_test` died on the first
//! `content_type.in?(ALLOWED_IMAGE_CONTENT_TYPES)`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn emitted(extra: &[(&str, &str)]) -> String {
    let mut files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"kind\", null: false\n  end\nend\n",
        ),
        (
            "app/models/post.rb",
            r#"class Post < ApplicationRecord
  ALLOWED = [ "text", "html" ]

  def allowed?
    kind.in?(ALLOWED)
  end
end
"#,
        ),
    ];
    files.extend_from_slice(extra);
    let mut app = ingest_app_from_tree(tree(&files)).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("post.rb"))
        .expect("no post.rb emitted")
        .content
        .clone()
}

#[test]
fn in_lowers_to_include_with_the_operands_swapped() {
    let src = emitted(&[]);
    assert!(src.contains("ALLOWED.include?(kind)"), "{src}");
    assert!(!src.contains(".in?("), "no in? may survive:\n{src}");
}

/// An app that defines its own `in?` means something else by the name,
/// and the pass stands down wholesale — the receiver of an `in?` is the
/// VALUE, so its type rarely names the class that defined the method
/// and a per-class check would not help.
#[test]
fn an_app_defining_its_own_in_predicate_stands_the_pass_down() {
    let src = emitted(&[(
        "app/models/thing.rb",
        "class Thing < ApplicationRecord\n  def in?(other)\n    false\n  end\nend\n",
    )]);
    assert!(src.contains(".in?(ALLOWED)"), "{src}");
}
