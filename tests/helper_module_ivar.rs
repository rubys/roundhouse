//! A CONTROLLER IVAR read inside a helper module body.
//!
//! Rails mixes helpers into the view instance, which carries the
//! controller's assigns — so `@room` in `RoomsHelper` IS the
//! controller's `@room`. Our helpers lower to module FUNCTIONS with no
//! instance, so the read was a bare nil and campfire's `rooms#show`
//! died on `undefined method 'id' for nil` inside
//! `link_to_edit_room`. lobsters' `StoriesHelper` reads `@user` and
//! `@ribbon` exactly the same way.
//!
//! Both halves are checked here, because either alone is broken: the
//! read routes through `ActionController::Current.controller` (the seam
//! `flash` / `cookies` / a request-reading `helper_method` already
//! use), and the base controller grows the reader it dispatches to.
//!
//! The reader goes on the BASE controller rather than the assigning
//! one: a helper runs under whichever controller is current, and Rails'
//! answer for an assign that controller never made is nil — which is
//! exactly what reading an unassigned ivar gives.

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

fn app(helper: &str) -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/rooms_controller.rb",
            "class RoomsController < ApplicationController\n  def show\n    @room = Room.find(params[:id])\n  end\nend\n",
        ),
        ("app/helpers/rooms_helper.rb", helper),
        ("app/views/rooms/show.html.erb", "<p><%= edit_label %></p>\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]))
    .expect("ingest")
}

const HELPER: &str = r#"module RoomsHelper
  def edit_label
    "edit-#{@room.id}"
  end
end
"#;

fn emitted(app: &roundhouse::App, suffix: &str) -> String {
    let mut files = ruby::emit_library(app);
    files.extend(ruby::emit_lowered_controllers(app));
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {suffix}; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

#[test]
fn a_helper_ivar_read_routes_through_the_live_controller() {
    let app = app(HELPER);
    let src = emitted(&app, "rooms_helper.rb");
    assert!(
        src.contains("ActionController::Current.controller.room"),
        "`@room` in a helper must reach the controller instance:\n{src}"
    );
    assert!(
        !src.contains("@room"),
        "no bare ivar read may survive — a module function has no instance:\n{src}"
    );
}

#[test]
fn the_base_controller_grows_the_reader_it_dispatches_to() {
    let app = app(HELPER);
    let src = emitted(&app, "application_controller.rb");
    assert!(
        src.contains("def room"),
        "ApplicationController must define the reader:\n{src}"
    );
}

/// A name the app already defines as a controller ACTION is left alone:
/// the synthesized reader would collide with it, and the app's own
/// method is the one that should win.
#[test]
fn a_name_the_controller_already_defines_is_not_claimed() {
    let src = emitted(
        &app("module RoomsHelper\n  def label\n    @show.to_s\n  end\nend\n"),
        "rooms_helper.rb",
    );
    assert!(
        src.contains("@show"),
        "`show` is an action on RoomsController — the ivar stays alone:\n{src}"
    );
}
