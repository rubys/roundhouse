//! A concern's methods run on its includer. With exactly one includer,
//! the analyzer types the module's bodies with that class as `self` —
//! so an association the includer declares resolves inside the concern
//! — and `lower::class_body_new` binds a class-side bare `new` to it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::class_body_new::apply_class_body_new_lowering;
use roundhouse::{ClassId, Symbol};

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "sessions", force: :cascade do |t|
    t.integer "user_id", null: false
    t.string "ip_address"
  end
  create_table "bans", force: :cascade do |t|
    t.integer "user_id", null: false
    t.string "ip_address"
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  include Bannable
  has_many :sessions
  has_many :bans
end
"#,
        ),
        (
            "app/models/session.rb",
            "class Session < ApplicationRecord\n  belongs_to :user\nend\n",
        ),
        (
            "app/models/ban.rb",
            "class Ban < ApplicationRecord\n  belongs_to :user\nend\n",
        ),
        (
            "app/models/user/bannable.rb",
            r#"module User::Bannable
  extend ActiveSupport::Concern

  class_methods do
    def banned_with(name)
      new(name: name)
    end
  end

  def ban_ips
    sessions.map { |s| s.ip_address }
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn a_concern_included_once_maps_to_its_includer() {
    let app = app();
    let map = app.sole_includer_of_modules();
    assert_eq!(
        map.get(&ClassId(Symbol::from("User::Bannable"))),
        Some(&ClassId(Symbol::from("User"))),
        "{map:?}"
    );
}

#[test]
fn a_concern_body_resolves_the_includers_associations() {
    // `sessions` is User's association; typed against the module it
    // resolved to nothing and the method answered untyped.
    let app = app();
    let lc = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "User::Bannable")
        .expect("concern is a library class");
    let m = lc.methods.iter().find(|m| m.name.as_str() == "ban_ips").expect("ban_ips");
    let ty = format!("{:?}", m.body.ty);
    assert!(ty.contains("Array") && ty.contains("Str"), "ban_ips typed {ty}");
}

#[test]
fn a_class_side_bare_new_in_a_concern_builds_the_includer() {
    let mut app = app();
    apply_class_body_new_lowering(&mut app);
    let lc = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "User::Bannable")
        .expect("concern");
    let m = lc.methods.iter().find(|m| m.name.as_str() == "banned_with").expect("banned_with");
    let body = format!("{:?}", m.body);
    assert!(body.contains("Const { path: [Symbol(\"User\")] }"), "{body}");
    assert!(!body.contains("Symbol(\"Bannable\")"), "{body}");
}
