//! Four sends that resolved in the EMIT and nowhere else, so the only
//! place they failed was the ledger the user reads.
//!
//! 1. **`Model.destroy_by(col: v)`** had no home at a class root at all:
//!    `Base` defines it for no target, and the arel pass claims neither
//!    it nor `delete_by`. Split into the `where` + `destroy_all` pair
//!    Rails defines it as.
//! 2. **`Random.uuid`** is `Random::Formatter#uuid`, the same method
//!    `SecureRandom.uuid` is — one module extended onto two classes.
//!    Nothing on any target defines the `Random` spelling.
//! 3. **A class method reached through a Relation** (`User.active
//!    .find_by_transfer_id(id)`) is re-rooted at the constant by the
//!    scope-chain survey, for ANY class method. The analyzer delegated
//!    only the relation-returning surface, leaving it stricter than the
//!    pipeline it describes.
//! 4. **`Model.insert_all(rows)`** is inlined by that same pass. Its
//!    catalog entry carried no return kind — which is not neutral: it
//!    falls through to the same place an unknown name does.
//!
//! Each test has a NEGATIVE twin where one exists, because a gate that
//! only asserts a clean ledger passes with the fix taken out.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n    t.boolean :active\n  end\n  \
    create_table :memberships do |t|\n    t.integer :user_id\n    t.integer :room_id\n  end\n\
    end\n";

const USER: &str = r#"
class User < ApplicationRecord
  scope :active, -> { where(active: true) }

  def self.find_by_transfer_id(id)
    find_by(id: id)
  end

  def grant_all(rows)
    Membership.insert_all(rows)
  end
end
"#;

const MEMBERSHIP: &str = "class Membership < ApplicationRecord\nend\n";

fn app_with(action_body: &str) -> roundhouse::App {
    let controller = format!(
        "class UsersController < ApplicationController\n  def index\n    {action_body}\n  end\nend\n"
    );
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n".to_string(),
        ),
        ("app/models/user.rb", USER.to_string()),
        ("app/models/membership.rb", MEMBERSHIP.to_string()),
        ("app/controllers/users_controller.rb", controller),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

/// Errors only — a `GradualUntyped` warning is a different ledger.
fn errors(action_body: &str, needle: &str) -> Vec<String> {
    diagnose(&app_with(action_body))
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && d.contains(needle))
        .collect()
}

fn emitted(action_body: &str) -> String {
    ruby::emit_spinel(&app_with(action_body))
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("users_controller.rb"))
        .map(|f| f.content)
        .expect("users_controller emitted")
}

#[test]
fn destroy_by_at_a_class_root_becomes_where_plus_destroy_all() {
    let o = errors("Membership.destroy_by(user_id: 1)", "destroy_by");
    assert!(o.is_empty(), "the split resolves the send: {o:?}");
    let src = emitted("Membership.destroy_by(user_id: 1)");
    assert!(
        src.contains(".destroy_all") && !src.contains(".destroy_by("),
        "and the emit carries the pair:\n{src}"
    );
}

#[test]
fn delete_by_takes_the_same_split() {
    let o = errors("Membership.delete_by(user_id: 1)", "delete_by");
    assert!(o.is_empty(), "the split resolves the send: {o:?}");
}

/// The negative twin: `where` is the whole rewrite, so a method the
/// class genuinely cannot answer must still be reported.
#[test]
fn a_name_no_model_answers_is_still_reported() {
    let o = errors("Membership.obliterate_by(user_id: 1)", "obliterate_by");
    assert_eq!(o.len(), 1, "an unknown class-side name still lands: {o:?}");
}

#[test]
fn random_uuid_is_securerandom_uuid() {
    let o = errors("@id = Random.uuid", "uuid");
    assert!(o.is_empty(), "the formatter method resolves: {o:?}");
    let src = emitted("@id = Random.uuid");
    assert!(
        src.contains("SecureRandom.uuid") && !src.contains("= Random.uuid"),
        "and the emit names the class that has a byte source:\n{src}"
    );
}

/// `rand` is a real method on the PRNG class with its own meaning.
/// Redirecting it would change behavior, so the pass must leave it.
#[test]
fn the_prng_surface_is_left_alone() {
    let src = emitted("@n = Random.rand(10)");
    assert!(
        src.contains("Random.rand"),
        "Random.rand is not a Formatter method:\n{src}"
    );
}

#[test]
fn a_class_method_reached_through_a_relation_resolves() {
    let o = errors("@u = User.active.find_by_transfer_id(1)", "find_by_transfer_id");
    assert!(
        o.is_empty(),
        "Rails runs it inside the relation's scoping block, and so do we: {o:?}"
    );
}

/// The negative twin for the widened delegation: it is gated on the
/// model DEFINING the name, so it must not become method_missing.
#[test]
fn a_relation_does_not_forward_a_name_nobody_defines() {
    let o = errors("@u = User.active.find_by_wishful_thinking(1)", "find_by_wishful_thinking");
    assert_eq!(o.len(), 1, "an undefined class-side name still lands: {o:?}");
}

#[test]
fn insert_all_answers_the_rows_it_was_given() {
    let o = errors("Membership.insert_all([{ user_id: 1 }])", "insert_all");
    assert!(o.is_empty(), "the inlined bulk write has a value: {o:?}");
}
