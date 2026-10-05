//! A bare `left_joins` at the head of a scope body must receive the
//! threaded relation, exactly like its `left_outer_joins` alias.
//!
//! Rails evaluates a scope lambda with `self` as the current relation, so
//! an admin dashboard writes a preload-and-count scope as:
//!
//! ```text
//! scope :with_sessions_count, -> {
//!   left_joins(:sessions)
//!     .select("users.*, COUNT(sessions.id) AS sessions_count")
//!     .group("users.id")
//! }
//! ```
//!
//! `left_outer_joins` was in the relation-chain table but its alias was
//! not, so the head call fell through the scope-body rewriter, stayed a
//! bare send, and the emitted method took its own fresh `__rel` default —
//! `left_joins` undefined on the class, which is a NoMethodError on
//! the page that calls the scope.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "email", null: false
    t.boolean "admin", default: false, null: false
  end
  create_table "sessions", force: :cascade do |t|
    t.bigint "user_id", null: false
    t.string "user_agent"
  end
end
"#;

const USER: &str = r#"class User < ApplicationRecord
  has_many :sessions

  scope :with_sessions_count, -> {
    left_joins(:sessions)
      .select("users.*, COUNT(sessions.id) AS sessions_count")
      .group("users.id")
  }
end
"#;

fn emitted() -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/user.rb"), USER.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("user.rb"))
        .expect("no user.rb emitted")
        .content
        .clone()
}

#[test]
fn the_bare_left_joins_scope_threads_the_relation() {
    let src = emitted();
    let at = src
        .find("def self.with_sessions_count")
        .unwrap_or_else(|| panic!("{src}"));
    let body = &src[at..src[at..].find("\n  end").map(|i| at + i).unwrap_or(src.len())];
    assert!(
        body.contains("__rel.left_joins("),
        "left_joins must chain onto the threaded relation:\n{body}"
    );
    assert!(
        body.contains("LEFT OUTER JOIN sessions"),
        "the association join must fold to a SQL fragment:\n{body}"
    );
}