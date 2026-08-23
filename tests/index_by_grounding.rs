//! `list.index_by { |x| key }` on a plain collection.
//!
//! ActiveSupport ships it as an `Enumerable` reopen, a shape only the
//! CRuby overlay can host. campfire builds `Sound::INDEX = BUILTIN
//! .index_by(&:name)` in a CLASS BODY, so an ungrounded call is not a
//! late NoMethodError on some route — it fires while `app/models.rb` is
//! being required and the tree does not boot. Sixth and last wall of a
//! stub-free campfire boot.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(src: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/post.rb"),
        b"class Post < ApplicationRecord
end
".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/sound.rb"), src.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.ends_with("sound.rb"))
        .expect("no sound.rb emitted")
        .content
}

#[test]
fn a_class_body_index_by_grounds_to_the_runtime_function() {
    let src = emitted(
        r#"class Sound
  BUILTIN = [ "a", "bb" ]
  INDEX = BUILTIN.index_by(&:length)
end
"#,
    );
    assert!(src.contains("ActiveSupport.index_by(BUILTIN)"), "{src}");
    // The receiver moved into argument position exactly once — a second
    // copy would evaluate a receiver with effects twice.
    assert_eq!(src.matches("BUILTIN").count(), 2, "{src}");
}

/// A Relation receiver keeps its own method: that one has a real RBS
/// signature, and routing it through the untyped module function would
/// trade a typed call for an untyped one to fix nothing.
#[test]
fn a_relation_receiver_keeps_the_typed_method() {
    let src = emitted(
        r#"class Sound
  def self.by_body
    Post.all.index_by(&:body)
  end
end
"#,
    );
    assert!(!src.contains("ActiveSupport.index_by"), "{src}");
    assert!(src.contains("index_by"), "{src}");
}
