//! Three test-body shapes campfire writes that the emit answered
//! wrongly:
//!
//! * `x_url(…, params: h)` — `params:` is `url_for`'s reserved option
//!   that IS the query. It rendered as a query key named `params`, or
//!   as a positional segment when the route's own segment had a
//!   default. Now the same split the erased `**splat` takes:
//!   `x_path(…) + RouteHelpers.query_suffix(h)`
//!   (`rewrites::route_helper_query_splat_index`).
//! * `record.save!(validate: false)` — a kwarg the zero-arity runtime
//!   `save!` cannot take. Now `save_after_validation`, the runtime's
//!   post-validation half (`lower::save_without_validation`).
//! * `owner.things.build(attrs)` where `has_many :things, class_name:
//!   "Some::Thing"` — the class came from the assoc name
//!   (`Thing`), an uninitialized constant. Now the declared target
//!   (`seeds_to_library::rewrite_assoc_create_with_models`).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(body: &str) -> Vec<(String, String)> {
    let src = format!(
        "require \"test_helper\"\n\nclass Users::PushSubscriptionsControllerTest < ActionDispatch::IntegrationTest\n  test \"one\" do\n    {body}\n  end\nend\n"
    );
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\n  create_table \"push_subscriptions\", force: :cascade do |t|\n    t.integer \"user_id\", null: false\n    t.string \"endpoint\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resources :users, only: %i[ show ] do\n    scope module: \"users\" do\n      resources :push_subscriptions, only: %i[ create ]\n    end\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/user.rb"),
            b"class User < ApplicationRecord\n  has_many :push_subscriptions, class_name: \"Push::Subscription\", dependent: :delete_all\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/push/subscription.rb"),
            b"class Push::Subscription < ApplicationRecord\n  belongs_to :user\n  validates :endpoint, presence: true\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/controllers/users/push_subscriptions_controller.rb"),
            b"class Users::PushSubscriptionsController < ApplicationController\n  def create\n    head :ok\n  end\nend\n".to_vec(),
        ),
        (PathBuf::from("test/fixtures/users.yml"), b"david:\n  name: David\n".to_vec()),
        (
            PathBuf::from("test/controllers/users/push_subscriptions_controller_test.rb"),
            src.into_bytes(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .iter()
        .map(|f| (f.path.to_string_lossy().to_string(), f.content.clone()))
        .collect()
}

fn file<'a>(files: &'a [(String, String)], suffix: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .unwrap_or_else(|| panic!("no {suffix} emitted"))
        .1
}

#[test]
fn a_params_option_on_a_route_helper_is_the_query_string() {
    let files = emitted(
        "post user_push_subscriptions_url(users(:david), params: { push_subscription: { endpoint: \"https://x\" } })\n    assert_response :ok",
    );
    let test = file(&files, "push_subscriptions_controller_test.rb");
    assert!(
        test.contains(
            "RouteHelpers.user_push_subscriptions_path(UsersFixtures.david.id) + RouteHelpers.query_suffix({ push_subscription: { endpoint: \"https://x\" } })"
        ),
        "`params:` should split off as the query suffix:\n{test}"
    );
    let helpers = file(&files, "app/route_helpers.rb");
    assert!(
        !helpers.contains("params: nil"),
        "no helper should grow a parameter named `params`:\n{helpers}"
    );
    assert!(
        helpers.contains("query = ActionView::ViewHelpers.to_query(params)"),
        "query_suffix renders through the runtime's to_query:\n{helpers}"
    );
}

#[test]
fn save_without_validation_enters_the_post_validation_half() {
    let files = emitted(
        "legacy = users(:david).push_subscriptions.build endpoint: \"https://x\"\n    legacy.save!(validate: false)\n    assert legacy.persisted?",
    );
    let test = file(&files, "push_subscriptions_controller_test.rb");
    assert!(test.contains("legacy.save_after_validation"), "{test}");
    assert!(!test.contains("validate: false"), "{test}");
}

#[test]
fn a_has_many_build_names_the_declared_class() {
    let files = emitted(
        "legacy = users(:david).push_subscriptions.build endpoint: \"https://x\"\n    assert legacy",
    );
    let test = file(&files, "push_subscriptions_controller_test.rb");
    assert!(
        test.contains("Push::Subscription.new(") || test.contains("::Push::Subscription.new("),
        "the build should name the association's class_name target:\n{test}"
    );
    assert!(!test.contains("PushSubscription.new"), "{test}");
}
