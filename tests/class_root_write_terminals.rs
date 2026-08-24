//! `<Model>.destroy_by(…)` / `.delete_by(…)` seed a Relation
//! (`scope_chain::CLASS_ROOT_TERMINALS`).
//!
//! Rails delegates every terminal from a model class to `all`. Here only
//! the ones that have no other home do that, because the arel pass owns
//! `count` / `exists?` / `find_by` / `all` at a Const root and re-rooting
//! those would move work out from under it (`Comment.count` is an
//! inlined `SELECT COUNT(*)`, not a Relation).
//!
//! `destroy_by` and `delete_by` have no such home: both are `where` plus
//! a write, `Base` defines neither, and the arel pass claims neither —
//! so campfire's `Push::Subscription.destroy_by(endpoint:, user_id:)`
//! reached NOTHING, with the analyzer saying so out loud
//! (`send_dispatch_failed: no known method `destroy_by` on
//! Class { Push::Subscription }`).
//!
//! The KWARGS form is now split earlier, by `lower::destroy_by`, into
//! the `where` + `destroy_all` / `delete_all` pair Rails defines it as —
//! which is what makes the analyzer see it resolve, and puts the `where`
//! where the arel pass can fold it. The Relation seed here still serves
//! every other shape `where` accepts, so both spellings are asserted:
//! the class root reaches a Relation either way, and never reaches
//! nothing.
//!
//! The GATE matters as much as the rewrite. `mentions_model_chain_start`
//! decides whether a body reaches the rewriter at all, and it asked only
//! about chain methods and `all` — so a body whose ONLY relation surface
//! is one of these terminals was never offered to the pass that exists
//! to fix it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(action_body: &str) -> String {
    let controller = format!(
        "class SubscriptionsController < ApplicationController\n  def destroy\n    {action_body}\n  end\nend\n"
    );
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"subscriptions\", force: :cascade do |t|\n    t.string \"endpoint\", null: false\n    t.integer \"user_id\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/subscription.rb"),
            b"class Subscription < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resource :subscription\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/controllers/subscriptions_controller.rb"),
            controller.into_bytes(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("subscriptions_controller.rb"))
        .map(|f| f.content)
        .expect("subscriptions_controller emitted")
}

#[test]
fn destroy_by_on_a_model_constant_seeds_a_relation() {
    let src = emitted("Subscription.destroy_by(endpoint: params[:endpoint])");
    assert!(
        src.contains("Subscription.where({ endpoint:")
            && src.contains(".destroy_all"),
        "the terminal rides a seeded Relation:\n{src}"
    );
}

#[test]
fn delete_by_takes_the_same_seed() {
    let src = emitted("Subscription.delete_by(endpoint: params[:endpoint])");
    assert!(
        src.contains("Subscription.where({ endpoint:") && src.contains(".delete_all"),
        "the terminal rides a seeded Relation:\n{src}"
    );
}

/// The shapes `lower::destroy_by` declines — a positional hash is one
/// `where` accepts and Rails' `destroy_by(*args)` forwards — still reach
/// a Relation, through the `CLASS_ROOT_TERMINALS` seed this file is
/// named for. Without it that call resolves to nothing at all.
#[test]
fn a_shape_the_kwargs_split_declines_still_seeds_a_relation() {
    let src = emitted("Subscription.destroy_by(\"endpoint IS NULL\")");
    assert!(
        src.contains("ActiveRecord::Relation.new(Subscription).destroy_by("),
        "the declined shape keeps the seed:\n{src}"
    );
}

/// The arel pass keeps what it owns: `count` at a Const root is an
/// inlined aggregate, not a Relation this pass re-roots.
#[test]
fn count_is_left_to_the_arel_pass() {
    let src = emitted("Subscription.count");
    assert!(
        !src.contains("ActiveRecord::Relation.new(Subscription).count"),
        "count keeps its inlined form:\n{src}"
    );
}
