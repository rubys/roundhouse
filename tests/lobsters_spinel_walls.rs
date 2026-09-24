//! The walls between upstream lobsters and a spinel C build, one pin
//! each, on one small app that has every shape.
//!
//! Each of these was a spinel refusal (or the `--rbs` seed contradiction
//! ahead of them) on `~/git/lobsters` at 0613f4d5..69df721c. They are
//! pinned on the emitted spinel tree because that is where each one
//! stopped the build; the shape each asserts is the one the refusal
//! needed gone.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "username", null: false
    t.text "settings"
  end
  create_table "origins", force: :cascade do |t|
    t.string "identifier", null: false
    t.integer "stories_count", default: 0, null: false
  end
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
    t.integer "origin_id"
  end
  create_table "hats", force: :cascade do |t|
    t.string "hat", null: false
  end
  create_table "comments", force: :cascade do |t|
    t.string "comment", null: false
    t.integer "hat_id"
  end
  create_table "domains", force: :cascade do |t|
    t.string "domain", null: false
  end
  create_table "mod_mails", force: :cascade do |t|
    t.string "subject", null: false
    t.string "short_id", null: false
  end
  create_table "mod_activities", force: :cascade do |t|
    t.integer "item_id"
    t.string "item_type"
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  root "users#index"
  resources :users
  resources :domains
  resources :mod_mails
  resources :mod_activities, only: [:index]
end
"#;

fn app_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  typed_store :settings do |s|\n    s.integer :avatar_source\n  end\n\n  def to_param\n    username\n  end\nend\n",
        ),
        (
            "app/models/origin.rb",
            "class Origin < ApplicationRecord\n  has_many :stories\n\n  after_create { Origin.reset_counters(id, :stories) }\nend\n",
        ),
        (
            "app/models/story.rb",
            "class Story < ApplicationRecord\n  belongs_to :origin, optional: true, counter_cache: true\nend\n",
        ),
        ("app/models/hat.rb", "class Hat < ApplicationRecord\nend\n"),
        (
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n  belongs_to :hat, optional: true\n  attr_accessor :current_vote\n\n  after_save :log_hat_use\n\n  def log_hat_use\n    return unless previously_new_record? || hat_previously_changed?\n    nil\n  end\n\n  def confidence\n    (BigDecimal(1) / BigDecimal(\"3\")).to_f\n  end\nend\n",
        ),
        ("app/models/domain.rb", "class Domain < ApplicationRecord\nend\n"),
        (
            "app/models/mod_mail.rb",
            "class ModMail < ApplicationRecord\n  def to_param\n    short_id\n  end\nend\n",
        ),
        (
            "app/models/mod_activity.rb",
            "class ModActivity < ApplicationRecord\n  belongs_to :item, polymorphic: true\nend\n",
        ),
        (
            "app/models/vote_hydrator.rb",
            "class VoteHydrator\n  def initialize(votes)\n    @votes = votes\n  end\n\n  def [](comment)\n    comment.current_vote ||= @votes[comment.id]\n    comment\n  end\nend\n",
        ),
        ("app/jobs/application_job.rb", "class ApplicationJob < ActiveJob::Base\nend\n"),
        (
            "app/jobs/warm_job.rb",
            "class WarmJob < ApplicationJob\n  def perform(path)\n    path.length\n  end\nend\n",
        ),
        (
            "app/jobs/prefill_job.rb",
            "class PrefillJob < ApplicationJob\n  def perform\n    jobs = paths.map { |path| WarmJob.new(path).set(queue: :default) }\n    ActiveJob.perform_all_later(jobs)\n  end\n\n  def paths\n    [\"/\", \"/active\"]\n  end\nend\n",
        ),
        (
            "app/jobs/backup_job.rb",
            "class BackupJob < ApplicationJob\n  def perform\n    system(\"sqlite3\", \"db.sqlite3\", \".backup x\", exception: true)\n  end\nend\n",
        ),
        (
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  def tag_link(tag)\n    \"<a>#{tag}</a>\"\n  end\n\n  def link_post(button_label, link)\n    render partial: \"helpers/link_post\", locals: {button_label: button_label, link: link}\n  end\nend\n",
        ),
        (
            "app/views/helpers/_link_post.html.erb",
            "<%# locals: (button_label:, link:, class_name: \"\") -%>\n<form action=\"<%= link %>\" class=\"<%= class_name %>\"><%= button_label %></form>\n",
        ),
        (
            "app/controllers/application_controller.rb",
            r#"class ApplicationController < ActionController::Base
  include ApplicationHelper

  CACHE_PAGE = proc { @user.blank? }

  rescue_from ActionController::ParameterMissing do |exception|
    respond_to do |format|
      format.html { render plain: "400 #{exception.message}", status: :bad_request }
      format.json { render json: {error: exception.message}, status: :bad_request }
    end
  end
end
"#,
        ),
        (
            "app/controllers/users_controller.rb",
            r#"class UsersController < ApplicationController
  def index
    @users = User.all
  end

  def create
    @user = User.new(user_params)
    if @user.save
      redirect_to root_path
    else
      redirect_back_or_to root_path
    end
  end

  private

  def user_params
    params.require(:user).permit(:username, :avatar_source)
  end
end
"#,
        ),
        (
            "app/controllers/domains_controller.rb",
            "class DomainsController < ApplicationController\n  before_action :find_or_initialize_domain\n\n  def edit\n  end\n\n  private\n\n  def find_or_initialize_domain\n    @domain = Domain.find_or_initialize_by(domain: params[:id])\n  end\nend\n",
        ),
        (
            "app/controllers/mod_activities_controller.rb",
            "class ModActivitiesController < ApplicationController\n  def index\n    @mod_activities = ModActivity.all\n  end\nend\n",
        ),
        ("app/views/users/index.html.erb", "<%= tag_link \"x\" %>\n"),
        ("app/views/domains/edit.html.erb", "<%= @domain.domain %>\n"),
        (
            "app/views/mod_activities/index.html.erb",
            "<% @mod_activities.each do |ma| %>\n  <% case ma.item %>\n  <% when ModMail %>\n    <%= link_to ma.item.subject, ma.item %>\n  <% end %>\n<% end %>\n",
        ),
    ]
}

fn emitted() -> HashMap<String, String> {
    let files: HashMap<PathBuf, Vec<u8>> = app_files()
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // The whole spinel tree, not just `emit_spinel`'s models/controllers/
    // views: helpers, jobs, POROs and the runtime ride the project build.
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    target_files(&app, &fixture, BuildTarget::Spinel).expect("spinel target files").into_iter().collect()
}

fn file<'a>(tree: &'a HashMap<String, String>, suffix: &str) -> &'a str {
    tree.iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| {
            let mut paths: Vec<&String> = tree.keys().collect();
            paths.sort();
            panic!("no {suffix} emitted; have {paths:#?}")
        })
}

/// `include ApplicationHelper` in the controller spliced the helper's
/// methods there, and the concern-body prune then emptied the module —
/// under every view that calls `ApplicationHelper.tag_link`.
#[test]
fn a_helper_module_the_controller_includes_keeps_its_methods() {
    let tree = emitted();
    let helper = file(&tree, "app/models/application_helper.rb");
    assert!(helper.contains("def self.tag_link"), "helper module emptied:\n{helper}");
}

/// A `rescue_from` handler answers the request the way an action does,
/// so its `respond_to` is flattened too — left whole it reached spinel
/// as a bare `respond_to` in every controller.
#[test]
fn a_rescue_from_handler_flattens_its_respond_to() {
    let tree = emitted();
    let users = file(&tree, "app/controllers/users_controller.rb");
    assert!(users.contains("rescue ActionController::ParameterMissing"), "{users}");
    assert!(!users.contains("respond_to"), "respond_to left in the handler:\n{users}");
}

/// A proc constant is an `instance_exec`'d filter condition; as a class
/// constant it reads instance state from no instance.
#[test]
fn a_proc_constant_on_a_controller_is_dropped() {
    let tree = emitted();
    let app_ctrl = file(&tree, "app/controllers/application_controller.rb");
    assert!(!app_ctrl.contains("CACHE_PAGE"), "{app_ctrl}");
}

/// A permitted param is a String; a typed_store integer's writer takes
/// an Integer — spinel's `--rbs seed contradicted` on `avatar_source=`.
#[test]
fn a_typed_store_integer_param_is_cast_in_the_typed_factory() {
    let tree = emitted();
    let user = file(&tree, "app/models/user.rb");
    assert!(user.contains("avatar_source = p.avatar_source.to_i"), "{user}");
}

/// `redirect_back_or_to` is a response terminal and a runtime method.
#[test]
fn redirect_back_or_to_is_a_terminal_the_runtime_defines() {
    let tree = emitted();
    let users = file(&tree, "app/controllers/users_controller.rb");
    assert!(users.contains("redirect_back_or_to(RouteHelpers.root_path)"), "{users}");
    let reopen = file(&tree, "runtime/redirect_back.rb");
    assert!(reopen.contains("def redirect_back_or_to"), "runtime lacks it");
    let boot = file(&tree, "boot.rb");
    assert!(boot.contains("require_relative \"runtime/redirect_back\""), "boot never loads it");
}

/// `find_or_initialize_by` in a one-statement method body is inlined
/// like its `where(…).first_or_initialize` spelling.
#[test]
fn find_or_initialize_by_in_a_one_statement_body_is_inlined() {
    let tree = emitted();
    let domains = file(&tree, "app/controllers/domains_controller.rb");
    assert!(!domains.contains("find_or_initialize_by"), "{domains}");
    assert!(domains.contains("_rec.domain = "), "{domains}");
}

/// Rails 7.1's belongs_to dirty predicate asks the foreign key.
#[test]
fn a_belongs_to_previously_changed_predicate_is_synthesized() {
    let tree = emitted();
    let comment = file(&tree, "app/models/comment.rb");
    assert!(comment.contains("def hat_previously_changed?"), "{comment}");
    let base = file(&tree, "runtime/active_record/base.rb");
    assert!(base.contains("def previously_new_record?"), "runtime lacks it");
}

/// Recounted through the has_many into `<assoc>_count`.
#[test]
fn reset_counters_recounts_into_the_counter_column() {
    let tree = emitted();
    let origin = file(&tree, "app/models/origin.rb");
    assert!(!origin.contains("reset_counters"), "{origin}");
    assert!(origin.contains("update_all(stories_count:"), "{origin}");
}

/// The bulk enqueue becomes one class-side enqueue per element.
#[test]
fn perform_all_later_over_a_map_enqueues_each() {
    let tree = emitted();
    let prefill = file(&tree, "app/models/prefill_job.rb");
    assert!(!prefill.contains("perform_all_later"), "{prefill}");
    assert!(prefill.contains("WarmJob.perform_later(path)"), "{prefill}");
}

/// `exception: true` is expressed without the options Hash.
#[test]
fn system_with_exception_true_raises_on_failure() {
    let tree = emitted();
    let backup = file(&tree, "app/models/backup_job.rb");
    assert!(!backup.contains("exception: true"), "{backup}");
    assert!(backup.contains("|| raise("), "{backup}");
}

/// `x.attr ||= v` on an untyped receiver is the read-or-write pair.
#[test]
fn an_attr_or_assign_on_an_untyped_receiver_is_desugared() {
    let tree = emitted();
    let hydrator = file(&tree, "app/models/vote_hydrator.rb");
    assert!(!hydrator.contains("||="), "{hydrator}");
    assert!(hydrator.contains("comment.current_vote || (comment.current_vote = "), "{hydrator}");
}

/// A helper rendering a strict-locals partial binds its locals as
/// keywords, and the partial's `link` local is the String the helper
/// hands it rather than the `Link`-by-name guess.
#[test]
fn a_helper_render_of_a_strict_locals_partial_binds_keywords() {
    let tree = emitted();
    let helper = file(&tree, "app/models/application_helper.rb");
    assert!(
        helper.contains("Views::Helpers.link_post(button_label, link: link)"),
        "{helper}"
    );
}

/// Inside `when ModMail`, `ma.item` IS a ModMail: the record link names
/// `mod_mail_path` and projects the model's own `to_param`.
#[test]
fn a_case_when_model_arm_links_through_that_models_route() {
    let tree = emitted();
    let view = file(&tree, "app/views/mod_activities/index.rb");
    assert!(!view.contains("item_path"), "{view}");
    assert!(view.contains("RouteHelpers.mod_mail_path(ma.item.to_param)"), "{view}");
}

/// `BigDecimal(...)` is spinel's bundled `packages/bigdecimal` (and
/// CRuby's gem); Rails loads it for the app, so the emitted file has to
/// say `require "bigdecimal"` itself or spinel refuses the call.
#[test]
fn a_file_naming_bigdecimal_requires_it() {
    let tree = emitted();
    let comment = file(&tree, "app/models/comment.rb");
    assert!(comment.starts_with("require \"bigdecimal\"\n"), "{comment}");
}

/// `Job.set(opts).perform_later(args)` folds to `Job.perform_later(args)`:
/// a class-side `set` would answer the class object, which the RBS
/// cannot name — lobsters' 13 jobs each failed cc on it. With no
/// unfolded `set` left, no job grows one.
#[test]
fn a_job_set_chain_folds_and_no_set_is_synthesized() {
    let tree = emitted();
    let prefill = file(&tree, "app/models/prefill_job.rb");
    assert!(!prefill.contains(".set("), "{prefill}");
    let warm = file(&tree, "app/models/warm_job.rb");
    assert!(!warm.contains("def self.set"), "{warm}");
    let warm_rbs = file(&tree, "app/models/warm_job.rbs");
    assert!(!warm_rbs.contains("def self.set"), "{warm_rbs}");
}
