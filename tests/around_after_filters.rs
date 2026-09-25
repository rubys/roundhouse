//! `around_action` and `after_action` in the synthesized dispatcher.
//!
//! Only `before_action` used to reach `process_action`; the other two
//! kinds were dropped without a word. Lobsters depends on both:
//! `around_action :track_story_reads, only: [:show], if: -> { @user.
//! present? }` loads the read ribbon its story page renders (the page
//! raised on a nil ribbon), `after_action :update_read_at` marks the
//! inbox read, and `after_action :clear_session_cookie` — skipped by
//! `skip_after_action` in the login controller, whose CSRF token lives
//! in that cookie — runs everywhere else.
//!
//! ActiveSupport's order: arounds nest in declaration order (first is
//! outermost), afters run in REVERSE declaration order after the action.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const APPLICATION_CONTROLLER: &str = r#"class ApplicationController < ActionController::Base
  after_action :clear_cookie

  private
    def clear_cookie
      @cleared = true
    end
end
"#;

const ROOMS: &str = r#"class RoomsController < ApplicationController
  around_action :track_reads, only: [:show], if: -> { @user.present? }
  after_action :first_after, only: [:index]
  after_action :second_after, only: [:index]
  after_action only: [:index] do
    @block_after = true
  end

  def index
  end

  def show
  end

  private
    def track_reads
      @ribbon = 1
      yield
      @ribbon = 2
    end

    def first_after
      @a = 1
    end

    def second_after
      @b = 1
    end
end
"#;

const SESSIONS: &str = r#"class SessionsController < ApplicationController
  skip_after_action :clear_cookie

  def new
  end
end
"#;

fn emitted() -> (String, String) {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"rooms\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/controllers/rooms_controller.rb", ROOMS),
        ("app/controllers/sessions_controller.rb", SESSIONS),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :rooms, only: [:index, :show]\n  get \"/session/new\" => \"sessions#new\"\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_controllers(&app);
    let get = |name: &str| {
        files
            .iter()
            .find(|f| f.path.to_string_lossy().ends_with(name))
            .map(|f| f.content.clone())
            .unwrap_or_else(|| panic!("{name}"))
    };
    (get("rooms_controller.rb"), get("sessions_controller.rb"))
}

#[test]
fn an_around_filter_wraps_the_dispatch_under_its_guards() {
    let (rooms, _) = emitted();
    assert!(
        rooms.contains("if [:show].include?(action_name) && @user.present?\n      self.track_reads do\n        case action_name"),
        "{rooms}"
    );
}

#[test]
fn after_filters_run_after_the_action_in_reverse_order() {
    let (rooms, _) = emitted();
    let second = rooms.find("second_after if").expect("second_after");
    let first = rooms.find("first_after if").expect("first_after");
    let block = rooms.find("@block_after = true").expect("block-form after");
    let inherited = rooms.find("clear_cookie").expect("inherited after");
    let dispatch_end = rooms.find("case action_name").expect("dispatch");
    assert!(dispatch_end < block && block < second && second < first && first < inherited, "{rooms}");
}

#[test]
fn skip_after_action_removes_the_inherited_after_filter() {
    let (_, sessions) = emitted();
    assert!(!sessions.contains("clear_cookie"), "{sessions}");
}
