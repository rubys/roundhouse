//! A method the RUNTIME answers must be in the catalog, or it reads as
//! a compiler gap.
//!
//! `Model.any?` / `Model.none?` — the unscoped emptiness predicates —
//! have been in `runtime/ruby/active_record/base.rb` for a while
//! (`count > 0` / `count == 0`, with the scoped forms going through
//! Relation beside them). The catalog did not carry them, so a call
//! typed to nothing and `analyze::diagnose` reported `no known method
//! any? on Class(User)`: modeling debt shown to the user as a defect.
//!
//! Companion to this fix, NOT gated here: `lower::params_merge`'s
//! synthesized `.to_attrs` send now carries its receiver's type, which
//! closed six more of the same false report on campfire. It has no unit
//! gate because the params-struct machinery it rides does not fire on a
//! minimal fixture — a three-file app produces no params class and no
//! `.to_attrs` at all, so a test written against one passes whether or
//! not the fix is there. The campfire emit ledger is what covers it.

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\nend\n";

const CONTROLLER: &str = r#"
class UsersController < ApplicationController
  def index
    @any = User.any?
    @none = User.none?
  end

  def destroy
    User.destroy_by(name: params[:name])
  end
end
"#;

#[test]
fn class_side_any_and_none_resolve() {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("app/controllers/users_controller.rb", CONTROLLER),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    let offenders: Vec<String> = diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && (d.contains("`any?`") || d.contains("`none?`")))
        .collect();
    assert!(
        offenders.is_empty(),
        "the runtime answers Model.any?/none?; the catalog must too: {offenders:?}"
    );
}

/// `Model.destroy_by(conditions)` — the same rule one step further on,
/// and with a wrinkle. `Relation#destroy_by` has been in
/// `runtime/ruby/active_record/relation.rb` all along; the UNSCOPED
/// form (campfire's `Push::Subscription.destroy_by(endpoint:, user_id:)`
/// on sign-out) had no class-side twin at all, so this fix is a runtime
/// method AND a catalog entry.
///
/// The wrinkle is the entry's shape: its neighbours `destroy_all` and
/// `delete_all` are catalogued with `return_kind: None`, which keeps the
/// effect ledger honest and still leaves a CALL typing to nothing. A
/// catalogued name is not by itself a resolved call.
#[test]
fn class_side_destroy_by_resolves() {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("app/controllers/users_controller.rb", CONTROLLER),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    let offenders: Vec<String> = diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && d.contains("`destroy_by`"))
        .collect();
    assert!(
        offenders.is_empty(),
        "the runtime answers Model.destroy_by; the catalog must too: {offenders:?}"
    );
}
