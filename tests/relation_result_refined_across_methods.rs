//! A method whose RESULT the class refines with a relation method keeps
//! its body on the Relation path.
//!
//! The arel pass lifts a recognized chain into an inline SELECT +
//! hydrate loop — an ARRAY. That is right when the consumer iterates it
//! and wrong when the consumer chains, which is why the pass already
//! declines to lift a chain assigned to a name the SAME BODY later
//! refines. A return value's consumer is in ANOTHER body, and the pass
//! sees one body at a time.
//!
//! campfire's `Autocompletable::UsersController` is the case:
//!
//! ```text
//!   def find_autocompletable_users = users_scope.active
//!   def users_scope = ... ? room.users : User.all
//! ```
//!
//! `User.all` lifted to a hydrate loop, and `.active` — a SCOPE, which
//! is why the builtin refiner list alone could not see this — then had
//! no relation left to chain: "undefined method 'active' for an
//! instance of Array", from a method whose source says `User.all`.

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

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
    t.integer "status", default: 0, null: false
  end
end
"#;

const MODEL: &str = r#"class User < ApplicationRecord
  scope :active, -> { where(status: 0) }
end
"#;

const CONTROLLER: &str = r#"class UsersController < ApplicationController
  def index
    @users = refined_scope
  end

  def listing
    @all = plain_scope
  end

  private

  def refined_scope
    users_scope.active
  end

  def users_scope
    User.all
  end

  def plain_scope
    User.all
  end
end
"#;

fn controller_src() -> String {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", MODEL),
        ("app/controllers/users_controller.rb", CONTROLLER),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("users_controller.rb"))
        .map(|f| f.content.clone())
        .expect("emitted controller")
}

fn method_body(src: &str, name: &str) -> String {
    let start = src
        .find(&format!("def {name}\n"))
        .unwrap_or_else(|| panic!("no `def {name}` in:\n{src}"));
    let rest = &src[start..];
    let end = rest.find("\n  end").map(|i| i + 5).unwrap_or(rest.len());
    rest[..end].to_string()
}

/// The SCOPE is what refines it, so the builtin refiner list is not
/// enough on its own — the app's relation-returning class methods come
/// from the analyzer's registry.
#[test]
fn a_method_whose_result_a_scope_refines_stays_a_relation() {
    let body = method_body(&controller_src(), "users_scope");
    assert!(
        body.contains("ActiveRecord::Relation.new(User)"),
        "the chain stays on the Relation path:\n{body}"
    );
    assert!(
        !body.contains("Db.prepare"),
        "no materializing hydrate loop:\n{body}"
    );
}

/// The guard is EVIDENCE-DRIVEN, not a blanket opt-out: a sibling
/// method nobody chains still lifts, which is the whole point of the
/// pass.
#[test]
fn an_unrefined_sibling_still_lifts() {
    let body = method_body(&controller_src(), "plain_scope");
    assert!(
        body.contains("Db.prepare"),
        "an unchained chain still materializes:\n{body}"
    );
}
