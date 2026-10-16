//! Rails' ParamsWrapper, decided per controller at lowering time
//! (`lower::controller_to_library::params_wrapper`) and applied per
//! request by `Params.wrap`. Request behavior against Rails is pinned in
//! `emit_and_run.rs`; these pin the decision: whether a controller wraps,
//! under which key, and which body keys it copies.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n    t.text :body\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  resources :articles, only: :create\n  \
    resources :sessions, only: :create\n  namespace :admin do\n    resources :articles, only: :create\n  end\nend\n";

const CREATE: &str = "  def create\n    head :ok\n  end\n";

struct App<'a> {
    files: Vec<(&'a str, String)>,
}

impl<'a> App<'a> {
    fn new(application_rb: &str) -> Self {
        App {
            files: vec![
                ("db/schema.rb", SCHEMA.to_string()),
                ("config/routes.rb", ROUTES.to_string()),
                ("config/application.rb", application_rb.to_string()),
                ("app/models/article.rb", "class Article < ApplicationRecord\nend\n".to_string()),
                ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n".to_string()),
                ("app/controllers/articles_controller.rb", format!("class ArticlesController < ApplicationController\n{CREATE}end\n")),
                ("app/controllers/sessions_controller.rb", format!("class SessionsController < ApplicationController\n{CREATE}end\n")),
                ("app/controllers/admin/articles_controller.rb", format!("module Admin\n  class ArticlesController < ApplicationController\n{CREATE}  end\nend\n")),
            ],
        }
    }

    fn with(mut self, path: &'a str, content: &str) -> Self {
        self.files.retain(|(p, _)| *p != path);
        self.files.push((path, content.to_string()));
        self
    }

    /// The emitted controller at `suffix` for `target`.
    fn controller(&self, target: BuildTarget, suffix: &str) -> String {
        let tree: HashMap<PathBuf, Vec<u8>> = self
            .files
            .iter()
            .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
            .collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        roundhouse::session::analyze_and_lower(&mut app);
        target_files(&app, Path::new("fixtures/tiny-blog"), target)
            .expect("emit")
            .into_iter()
            .find(|(p, _)| p.ends_with(suffix))
            .map(|(_, c)| c)
            .unwrap_or_else(|| panic!("no {suffix}"))
    }

    fn ruby(&self, suffix: &str) -> String {
        self.controller(BuildTarget::Ruby, suffix)
    }
}

const RAILS_8: &str = "module Blog\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n";
const RAILS_6: &str = "module Blog\n  class Application < Rails::Application\n    config.load_defaults 6.1\n  end\nend\n";

/// Rails' `attribute_names` includes the primary key.
const ARTICLE_WRAP: &str =
    r#"@params = Params.wrap(@params, request, "article", true, ["id", "title", "body"], [])"#;

#[test]
fn load_defaults_7_or_later_wraps_under_the_model_name() {
    let src = App::new(RAILS_8).ruby("app/controllers/articles_controller.rb");
    let wrap = src.find("Params.wrap").unwrap_or_else(|| panic!("no wrap:\n{src}"));
    assert!(src.contains(ARTICLE_WRAP), "{src}");
    // First in the dispatcher, ahead of the action dispatch, as Rails
    // runs it outside every callback.
    let case = src.find("case action_name").expect("dispatch");
    assert!(wrap < case, "{src}");
}

#[test]
fn older_defaults_do_not_wrap() {
    let src = App::new(RAILS_6).ruby("app/controllers/articles_controller.rb");
    assert!(!src.contains("Params.wrap"), "{src}");
}

#[test]
fn the_pre_7_initializer_wraps() {
    let src = App::new(RAILS_6)
        .with(
            "config/initializers/wrap_parameters.rb",
            "ActiveSupport.on_load(:action_controller) do\n  wrap_parameters format: [:json]\nend\n",
        )
        .ruby("app/controllers/articles_controller.rb");
    assert!(src.contains(ARTICLE_WRAP), "{src}");
}

#[test]
fn an_explicit_config_key_wins_over_load_defaults() {
    let src = App::new(RAILS_8)
        .with(
            "config/initializers/new_framework_defaults.rb",
            "Rails.application.config.action_controller.wrap_parameters_by_default = false\n",
        )
        .ruby("app/controllers/articles_controller.rb");
    assert!(!src.contains("Params.wrap"), "{src}");
}

#[test]
fn parenthesized_load_defaults_wraps() {
    let src = App::new(
        "module Blog\n  class Application < Rails::Application\n    config.load_defaults(8.1)\n  end\nend\n",
    )
    .ruby("app/controllers/articles_controller.rb");
    assert!(src.contains(ARTICLE_WRAP), "{src}");
}

/// A controller-scoped call in an initializer is that controller's, not
/// the app-wide on_load default — wrapping stays off under Rails 6.
#[test]
fn a_controller_receiver_in_an_initializer_is_not_the_app_default() {
    let src = App::new(RAILS_6)
        .with(
            "config/initializers/wrap_parameters.rb",
            "ArticlesController.wrap_parameters format: [:json]\n",
        )
        .ruby("app/controllers/sessions_controller.rb");
    assert!(!src.contains("Params.wrap"), "{src}");
}

/// The pre-7 generator's `format: [:json]` alone is the app default;
/// extra options on that line are not modeled as a silent global wrap.
#[test]
fn an_initializer_format_with_include_is_not_the_app_default() {
    let src = App::new(RAILS_6)
        .with(
            "config/initializers/wrap_parameters.rb",
            "ActiveSupport.on_load(:action_controller) do\n  wrap_parameters format: [:json], include: [:token]\nend\n",
        )
        .ruby("app/controllers/articles_controller.rb");
    assert!(!src.contains("Params.wrap"), "{src}");
}

/// Rails enables wrapping only when the format list contains `:json`;
/// `:json_api` is a different mime and must not flip the app default.
#[test]
fn an_initializer_json_api_format_is_not_the_app_default() {
    let src = App::new(RAILS_6)
        .with(
            "config/initializers/wrap_parameters.rb",
            "ActiveSupport.on_load(:action_controller) do\n  wrap_parameters format: [:json_api]\nend\n",
        )
        .ruby("app/controllers/articles_controller.rb");
    assert!(!src.contains("Params.wrap"), "{src}");
}

#[test]
fn a_parent_switching_it_off_is_inherited_and_a_child_can_switch_it_back() {
    let app = App::new(RAILS_8).with(
        "app/controllers/application_controller.rb",
        "class ApplicationController < ActionController::API\n  wrap_parameters false\nend\n",
    );
    assert!(!app.ruby("app/controllers/articles_controller.rb").contains("Params.wrap"));
    let src = app
        .with(
            "app/controllers/articles_controller.rb",
            &format!("class ArticlesController < ApplicationController\n  wrap_parameters format: [:json]\n{CREATE}end\n"),
        )
        .ruby("app/controllers/articles_controller.rb");
    assert!(src.contains(ARTICLE_WRAP), "{src}");
}

/// An unreadable ancestor call leaves the format unknown, but a child
/// that sets `format:` replaces the options in Rails — so wrapping is
/// decidable again for that controller alone.
#[test]
fn a_child_format_call_overrides_an_unreadable_ancestor() {
    let app = App::new(RAILS_8).with(
        "app/controllers/application_controller.rb",
        "class ApplicationController < ActionController::API\n  wrap_parameters wrapper_options\nend\n",
    );
    assert!(
        !app.ruby("app/controllers/sessions_controller.rb").contains("Params.wrap"),
        "siblings without a clarifying call stay unwrapped"
    );
    let src = app
        .with(
            "app/controllers/articles_controller.rb",
            &format!("class ArticlesController < ApplicationController\n  wrap_parameters format: [:json]\n{CREATE}end\n"),
        )
        .ruby("app/controllers/articles_controller.rb");
    assert!(src.contains(ARTICLE_WRAP), "{src}");
}

#[test]
fn a_name_and_include_replace_the_defaults() {
    let src = App::new(RAILS_8)
        .with(
            "app/controllers/articles_controller.rb",
            &format!("class ArticlesController < ApplicationController\n  wrap_parameters :post, include: [:title]\n{CREATE}end\n"),
        )
        .ruby("app/controllers/articles_controller.rb");
    assert!(src.contains(r#"Params.wrap(@params, request, "post", true, ["title"], [])"#), "{src}");
}

#[test]
fn without_a_model_every_body_key_but_exclude_is_copied() {
    let src = App::new(RAILS_8)
        .with(
            "app/controllers/sessions_controller.rb",
            &format!("class SessionsController < ApplicationController\n  wrap_parameters exclude: [:secret]\n{CREATE}end\n"),
        )
        .ruby("app/controllers/sessions_controller.rb");
    assert!(src.contains(r#"Params.wrap(@params, request, "session", false, [], ["secret"])"#), "{src}");
}

#[test]
fn a_namespaced_controller_finds_the_model_by_dropping_the_namespace() {
    let src = App::new(RAILS_8).ruby("app/controllers/admin/articles_controller.rb");
    assert!(src.contains(ARTICLE_WRAP), "{src}");
}

#[test]
fn aliases_and_nested_attributes_join_the_attribute_names() {
    let src = App::new(RAILS_8)
        .with(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments\n  alias_attribute :headline, :title\n  accepts_nested_attributes_for :comments\nend\n",
        )
        .ruby("app/controllers/articles_controller.rb");
    assert!(
        src.contains(r#""article", true, ["id", "title", "body", "headline", "comments_attributes"], [])"#),
        "{src}"
    );
}

#[test]
fn strict_targets_do_not_wrap() {
    // Crystal stands in for every target outside the ruby family.
    let src = App::new(RAILS_8).controller(BuildTarget::Crystal, "articles_controller.cr");
    assert!(!src.contains("wrap"), "{src}");
}
