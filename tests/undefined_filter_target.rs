//! A filter whose target method nothing defines (`analyze::filter_targets`).
//!
//! Hifumi's generated Event RSVP app wrote `before_action
//! :authenticate_user!` with no Devise in the Gemfile. Rails loads the
//! class and answers every guarded action with a 500. `check` said 0
//! errors, and the lowering dropped the filter, so the emitted app ran
//! `new`/`create` unauthenticated. The cases below are the places a
//! target legitimately lives, each of which must stay quiet.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :events do |t|\n    t.string :title\n  end\n  \
    create_table :users do |t|\n    t.string :email\n  end\nend\n";

const BASE: &[(&str, &str)] = &[
    ("db/schema.rb", SCHEMA),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\n  resources :events, only: [:index, :new]\nend\n",
    ),
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    ),
    ("app/models/event.rb", "class Event < ApplicationRecord\nend\n"),
    ("app/views/events/index.html.erb", "<p>events</p>\n"),
    ("app/views/events/new.html.erb", "<p>new</p>\n"),
];

const APPLICATION_CONTROLLER: &str =
    "class ApplicationController < ActionController::Base\nend\n";

fn events_controller(filter: &str) -> String {
    format!(
        "class EventsController < ApplicationController\n  {filter}\n\n  \
         def index\n  end\n\n  def new\n  end\nend\n"
    )
}

/// `(code, target)` for every undefined-filter-target error.
fn undefined_targets(extra: &[(&str, &str)]) -> Vec<String> {
    let mut files: HashMap<&str, &str> = BASE.iter().copied().collect();
    files.insert("app/controllers/application_controller.rb", APPLICATION_CONTROLLER);
    files.extend(extra.iter().copied());
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    diagnose(&app)
        .into_iter()
        .filter_map(|d| match &d.kind {
            DiagnosticKind::UndefinedFilterTarget { target, .. } => {
                assert_eq!(d.severity, Severity::Error, "{d}");
                assert_eq!(d.code(), "undefined_filter_target");
                Some(target.as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_before_action_naming_nothing_is_an_error() {
    let controller =
        events_controller("before_action :authenticate_user!, except: %i[index show]");
    let found = undefined_targets(&[("app/controllers/events_controller.rb", &controller)]);
    assert_eq!(found, ["authenticate_user!"]);
}

#[test]
fn around_and_after_actions_are_checked_too() {
    let controller = events_controller("around_action :with_locale\n  after_action :track");
    let found = undefined_targets(&[("app/controllers/events_controller.rb", &controller)]);
    assert_eq!(found, ["with_locale", "track"]);
}

#[test]
fn a_target_the_controller_defines_is_fine() {
    let controller = "class EventsController < ApplicationController\n  \
        before_action :set_title\n\n  def index\n  end\n\n  def new\n  end\n\n  \
        private\n\n  def set_title\n    @title = \"Events\"\n  end\nend\n";
    let found = undefined_targets(&[("app/controllers/events_controller.rb", controller)]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_target_an_ancestor_or_its_concern_defines_is_fine() {
    let concern = "module Authentication\n  extend ActiveSupport::Concern\n\n  \
        private\n\n  def require_login\n    redirect_to root_path unless session[:user_id]\n  end\nend\n";
    let app_controller = "class ApplicationController < ActionController::Base\n  \
        include Authentication\n\n  private\n\n  def audit\n  end\nend\n";
    let controller = events_controller("before_action :require_login\n  after_action :audit");
    let found = undefined_targets(&[
        ("app/controllers/concerns/authentication.rb", concern),
        ("app/controllers/application_controller.rb", app_controller),
        ("app/controllers/events_controller.rb", &controller),
    ]);
    assert!(found.is_empty(), "{found:?}");
}

/// A base declares the filter; each subclass supplies the method. Rails
/// looks it up per action, so this works.
#[test]
fn a_target_a_subclass_defines_is_fine() {
    let app_controller = "class ApplicationController < ActionController::Base\n  \
        before_action :load_resource\nend\n";
    let controller = "class EventsController < ApplicationController\n  \
        def index\n  end\n\n  def new\n  end\n\n  private\n\n  \
        def load_resource\n    @event = Event.new\n  end\nend\n";
    let found = undefined_targets(&[
        ("app/controllers/application_controller.rb", app_controller),
        ("app/controllers/events_controller.rb", controller),
    ]);
    assert!(found.is_empty(), "{found:?}");
}

/// Devise generates `authenticate_<scope>!` from the model's `devise`
/// declaration; the registry already gives controllers that surface.
#[test]
fn devise_scope_helpers_resolve() {
    let user = "class User < ApplicationRecord\n  devise :database_authenticatable\nend\n";
    let controller = events_controller("before_action :authenticate_user!");
    let found = undefined_targets(&[
        ("app/models/user.rb", user),
        ("app/controllers/events_controller.rb", &controller),
    ]);
    assert!(found.is_empty(), "{found:?}");
}

/// A module the app does not define (here a gem's), or one an app
/// concern pulls in, could define anything: the absence proves nothing.
#[test]
fn an_include_out_of_sight_silences_the_check() {
    let direct = "class ApplicationController < ActionController::Base\n  \
        include Pundit::Authorization\nend\n";
    let controller = events_controller("after_action :verify_authorized");
    let found = undefined_targets(&[
        ("app/controllers/application_controller.rb", direct),
        ("app/controllers/events_controller.rb", &controller),
    ]);
    assert!(found.is_empty(), "{found:?}");

    let concern = "module Authorization\n  extend ActiveSupport::Concern\n  \
        include Pundit::Authorization\nend\n";
    let via_concern = "class ApplicationController < ActionController::Base\n  \
        include Authorization\nend\n";
    let found = undefined_targets(&[
        ("app/controllers/concerns/authorization.rb", concern),
        ("app/controllers/application_controller.rb", via_concern),
        ("app/controllers/events_controller.rb", &controller),
    ]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn framework_callbacks_resolve() {
    let controller = events_controller("before_action :verify_authenticity_token");
    let found = undefined_targets(&[("app/controllers/events_controller.rb", &controller)]);
    assert!(found.is_empty(), "{found:?}");
}

/// A superclass that is not an app controller nor the framework base (a
/// gem's controller) carries methods nothing here can see.
#[test]
fn a_gem_superclass_silences_the_check() {
    let controller = "class EventsController < Devise::SessionsController\n  \
        before_action :configure_sign_in_params\n\n  def index\n  end\n\n  def new\n  end\nend\n";
    let found = undefined_targets(&[("app/controllers/events_controller.rb", controller)]);
    assert!(found.is_empty(), "{found:?}");
}
