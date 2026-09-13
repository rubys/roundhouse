//! `allow_browser` (`ingest::allow_browser`): the macro becomes a
//! `before_action :allow_browser` plus the private method that asks
//! `ActionController::BrowserBlocker` and runs the fallback — from a
//! concern's `included do` (campfire's shape, with its own floors and
//! its own page) and from a controller body (the Rails 7.2+ generator's
//! `allow_browser versions: :modern`).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(concern: Option<&str>, application_controller: &str) -> String {
    emitted_file(concern, application_controller, "app/controllers/application_controller.rb")
}

fn emitted_file(concern: Option<&str>, application_controller: &str, suffix: &str) -> String {
    let mut files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"sessions\", force: :cascade do |t|\n    t.string \"token\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resource :session, only: %i[ new ]\nend\n".to_vec(),
        ),
        (PathBuf::from("app/models/session.rb"), b"class Session < ApplicationRecord\nend\n".to_vec()),
        (
            PathBuf::from("app/controllers/application_controller.rb"),
            application_controller.as_bytes().to_vec(),
        ),
        (
            PathBuf::from("app/controllers/sessions_controller.rb"),
            b"class SessionsController < ApplicationController\n  def new\n  end\nend\n".to_vec(),
        ),
        (PathBuf::from("app/views/sessions/new.html.erb"), b"<h1>Sign in</h1>\n".to_vec()),
        (
            PathBuf::from("app/views/sessions/incompatible_browser.html.erb"),
            b"<h1>Upgrade to a supported web browser</h1>\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    if let Some(src) = concern {
        files.insert(
            PathBuf::from("app/controllers/concerns/allow_browser.rb"),
            src.as_bytes().to_vec(),
        );
    }
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("no {suffix} emitted"))
        .content
        .clone()
}

#[test]
fn a_concern_included_macro_becomes_a_gate_method_and_a_before_action() {
    let src = emitted(
        Some(
            "module AllowBrowser\n  extend ActiveSupport::Concern\n\n  VERSIONS = { safari: 17.2, chrome: 120, firefox: 121, opera: 104, ie: false }\n\n  included do\n    allow_browser versions: VERSIONS, block: -> { render template: \"sessions/incompatible_browser\" }\n  end\nend\n",
        ),
        "class ApplicationController < ActionController::Base\n  include AllowBrowser\nend\n",
    );
    assert!(
        src.contains("def allow_browser"),
        "the gate method should be spliced from the concern:\n{src}"
    );
    assert!(
        src.contains(
            "ActionController::BrowserBlocker.blocked?(request.user_agent, { \"safari\" => \"17.2\", \"chrome\" => \"120\", \"firefox\" => \"121\", \"opera\" => \"104\", \"ie\" => \"false\" })"
        ),
        "the floors travel as strings:\n{src}"
    );
    assert!(
        src.contains("Views::Sessions.incompatible_browser("),
        "the fallback's slashed template resolves from ApplicationController:\n{src}"
    );
    let sessions = emitted_file(
        Some(
            "module AllowBrowser\n  extend ActiveSupport::Concern\n\n  VERSIONS = { safari: 17.2, chrome: 120, firefox: 121, opera: 104, ie: false }\n\n  included do\n    allow_browser versions: VERSIONS, block: -> { render template: \"sessions/incompatible_browser\" }\n  end\nend\n",
        ),
        "class ApplicationController < ActionController::Base\n  include AllowBrowser\nend\n",
        "app/controllers/sessions_controller.rb",
    );
    assert!(
        sessions.contains("allow_browser\n    return if self.performed?"),
        "the inheritor's process_action runs the gate and halts on its render:\n{sessions}"
    );
}

#[test]
fn the_generator_form_is_left_as_written_until_every_request_answers_user_agent() {
    // real-blog carries `allow_browser versions: :modern` and is emitted
    // for twelve targets; only the ruby family's request has
    // `user_agent`, so this form stays a dropped class-body call for
    // now (see `ingest::allow_browser`).
    let src = emitted(
        None,
        "class ApplicationController < ActionController::Base\n  allow_browser versions: :modern\nend\n",
    );
    assert!(!src.contains("BrowserBlocker"), "{src}");
}
