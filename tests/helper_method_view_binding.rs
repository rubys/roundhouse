//! `helper_method :name` — the app declaring which controller methods a
//! view may call.
//!
//! Our views lower to module FUNCTIONS with no controller instance, so a
//! bare `platform` in a template resolved to nothing and campfire's room
//! page died on `NameError: undefined local variable or method
//! 'platform' for module Views::Pwa`.
//!
//! There are two arms, and the discriminator already existed.
//! `controller_helper_method_names` clones a marked method CLASS-SIDE
//! when its body is pure over its arguments, and the view calls
//! `DomainsController.caption_of_button(domain)` — a static call, which
//! is right because there is no per-request state to reach. A marked
//! method that READS REQUEST STATE cannot be cloned for exactly that
//! reason, and used to be left as residue. Those route through the live
//! controller, the seam `flash` and `cookies` already use.
//!
//! Ingest also had to learn the CONCERN spelling: the existing scan
//! reads `helper_method` from a controller class body, and campfire
//! writes all three of its declarations inside a concern's `included
//! do`.

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
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

/// The campfire shape: declared in a CONCERN's `included do`, and the
/// body reads request state, so it cannot be cloned class-side.
const SET_PLATFORM: &str = r#"module SetPlatform
  extend ActiveSupport::Concern

  included do
    helper_method :platform
  end

  def platform
    @platform ||= request.user_agent
  end
end
"#;

/// The lobsters shape: declared in the controller body, ARG-PURE, so it
/// keeps its class-side clone.
const CONTROLLER: &str = r#"class RoomsController < ApplicationController
  include SetPlatform

  helper_method :caption_of

  def show
    @room = Room.find(params[:id])
  end

  def caption_of(room)
    "Room #{room.name}"
  end
end
"#;

fn app(view: &str) -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/concerns/set_platform.rb", SET_PLATFORM),
        ("app/controllers/rooms_controller.rb", CONTROLLER),
        ("app/views/rooms/show.html.erb", view),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]))
    .expect("ingest")
}

fn view_body(view: &str) -> String {
    let app = app(view);
    let files = ruby::emit_lowered_views(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("rooms/show.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted rooms/show; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

/// The gap this closes: a marked method whose body reads request state.
/// It cannot be cloned class-side, so it routes through the live
/// controller.
#[test]
fn a_request_reading_helper_method_routes_through_the_controller() {
    let body = view_body("<p><%= platform %></p>\n");
    assert!(
        body.contains("ActionController::Current.controller.platform"),
        "bare `platform` must reach the controller instance:\n{body}"
    );
}

/// …and the declaration is read from the CONCERN, which is where
/// campfire writes all three of its `helper_method` calls.
#[test]
fn the_declaration_is_read_from_a_concerns_included_block() {
    let app = app("<p><%= platform %></p>\n");
    assert!(
        app.view_visible_controller_methods
            .iter()
            .any(|m| m.as_str() == "platform"),
        "`helper_method :platform` inside `included do` must register: {:?}",
        app.view_visible_controller_methods
    );
}

/// The arm that already worked must not be taken. An ARG-PURE marked
/// method has a class-side clone, and rewriting it to a dynamic
/// `Current.controller` call is a regression — it is how this change
/// first broke lobsters' `DomainsController.caption_of_button`.
#[test]
fn an_arg_pure_helper_method_keeps_its_static_call() {
    let body = view_body("<p><%= caption_of(room) %></p>\n");
    assert!(
        !body.contains("ActionController::Current.controller.caption_of"),
        "the class-side clone serves this one:\n{body}"
    );
    assert!(
        body.contains("RoomsController.caption_of"),
        "and the view calls it statically:\n{body}"
    );
}

/// A template local of the same name IS that local — Rails mixes
/// helpers BENEATH a template's locals, and the emitted view takes them
/// as parameters. Without the guard, a partial declaring `platform:`
/// would ignore what its caller passed and read the controller.
#[test]
fn a_template_local_of_the_same_name_shadows_the_routing() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/concerns/set_platform.rb", SET_PLATFORM),
        ("app/controllers/rooms_controller.rb", CONTROLLER),
        (
            "app/views/rooms/_badge.html.erb",
            "<%# locals: (platform:) %>\n<p><%= platform %></p>\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]))
    .expect("ingest");
    let files = ruby::emit_lowered_views(&app);
    let badge = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("rooms/_badge.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted _badge; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        });
    assert!(
        badge.contains("def self.badge(platform)"),
        "the local is a parameter:\n{badge}"
    );
    assert!(
        !badge.contains("ActionController::Current.controller.platform"),
        "and the parameter wins over the helper_method:\n{badge}"
    );
}

/// `params` in a view is the same seam one name over — a bare reference
/// that resolved to nothing. lobsters' password-reset page read
/// `params[:token]` from a module function that never declared it.
#[test]
fn params_in_a_view_reaches_the_controller() {
    let body = view_body("<p><%= params[:id] %></p>\n");
    assert!(
        body.contains("ActionController::Current.controller.params"),
        "bare `params` must reach the controller instance:\n{body}"
    );
}
