//! A before_action inlined into an action body keeps Rails' halting
//! semantics — and its position in the chain.
//!
//! `inline_before_filters` prepends a filter target's body to each
//! action that fires it, so the body-typer sees the `@room = …`
//! assignment and types downstream reads. Two things it was losing:
//!
//! 1. **The halt.** Rails skips the action when a filter renders or
//!    redirects. An inlined filter that halts the Rails way — `head`,
//!    `render`, `redirect_to`, no explicit `return` — left the action
//!    running. campfire's `ensure_can_administer` emitted
//!    `head(:forbidden)` and then `@room.destroy` ran anyway: a
//!    non-administrator got a 403 AND the room was deleted.
//!
//! 2. **The order.** Inlining moves a filter INTO the action, and the
//!    preamble runs BEFORE the action — so an inlined filter always
//!    runs after every preamble one, whatever the source said. That is
//!    fine while a controller's own inlinable filters are all declared
//!    before its non-inlinable ones, and wrong the moment they are not.

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

const APPLICATION_CONTROLLER: &str = r#"class ApplicationController < ActionController::Base
  private
    def note_the_visit
      @visited = true
    end
end
"#;

fn app(controller: &str) -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\nend\n",
        ),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/controllers/rooms_controller.rb", controller),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]))
    .expect("ingest")
}

fn emitted(app: &roundhouse::App) -> String {
    let files = ruby::emit_lowered_controllers(app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("rooms_controller.rb"))
        .map(|f| f.content.clone())
        .expect("rooms_controller")
}

/// Own-private filters only, in a safe order: inlining stands, and the
/// authorization filter's body is followed by the halt.
const ORDERED: &str = r#"class RoomsController < ApplicationController
  before_action :set_room, only: %i[ show destroy ]
  before_action :ensure_can_administer, only: %i[ destroy ]

  def show
  end

  def destroy
    @room.destroy
  end

  private
    def set_room
      @room = Room.find_by(id: params[:id])
    end

    def ensure_can_administer
      head :forbidden unless @room.name == "open"
    end
end
"#;

/// An ancestor-targeted filter declared AFTER an inlinable one — the
/// campfire shape. Inlining has to decline for the whole controller.
const OUT_OF_ORDER: &str = r#"class RoomsController < ApplicationController
  before_action :set_room, only: %i[ show destroy ]
  before_action :ensure_can_administer, only: %i[ destroy ]
  before_action :note_the_visit, only: :show

  def show
  end

  def destroy
    @room.destroy
  end

  private
    def set_room
      @room = Room.find_by(id: params[:id])
    end

    def ensure_can_administer
      head :forbidden unless @room.name == "open"
    end
end
"#;

/// The security case: a filter that responds must stop the action.
/// Without the guard `@room.destroy` ran after the 403.
#[test]
fn an_inlined_filter_that_responds_halts_the_action() {
    let out = emitted(&app(ORDERED));
    let destroy = out
        .split("def destroy")
        .nth(1)
        .expect("destroy body")
        .split("\n  end")
        .next()
        .unwrap()
        .to_string();
    let halt = destroy.find("return if self.performed?").expect(
        &format!("destroy must halt after the authorization filter:\n{destroy}"),
    );
    let del = destroy.find("@room.destroy").expect("the action body");
    assert!(
        halt < del,
        "the halt has to come BEFORE the action's own work:\n{destroy}"
    );
}

/// A filter that only assigns adds no dispatch noise — the guard is
/// scoped to filters that can actually respond.
#[test]
fn a_pure_assignment_filter_adds_no_halt() {
    let out = emitted(&app(ORDERED));
    let show = out
        .split("def show")
        .nth(1)
        .expect("show body")
        .split("\n  end")
        .next()
        .unwrap()
        .to_string();
    assert!(
        !show.contains("return if self.performed?"),
        "`set_room` cannot respond, so `show` needs no guard:\n{show}"
    );
    assert!(
        show.contains("@room = Room.find_by"),
        "and it is still inlined, which is what types `@room`:\n{show}"
    );
}

/// The order case: with a non-inlinable filter declared last, every own
/// filter falls through to the preamble, where declaration order is
/// what Rails runs.
#[test]
fn a_later_ancestor_filter_declines_inlining_for_the_controller() {
    let out = emitted(&app(OUT_OF_ORDER));
    let dispatch = out
        .split("def process_action")
        .nth(1)
        .expect("process_action")
        .split("\n  end")
        .next()
        .unwrap()
        .to_string();
    let set_room = dispatch.find("set_room").expect("set_room in the preamble");
    let admin = dispatch.find("ensure_can_administer").expect("ensure_can_administer");
    let visit = dispatch.find("note_the_visit").expect("note_the_visit");
    assert!(
        set_room < admin && admin < visit,
        "declaration order is what Rails runs:\n{dispatch}"
    );
    assert!(
        !out.contains("@room = Room.find_by(id: @params[\"id\"])\n    @room.destroy"),
        "and nothing is inlined into the action bodies:\n{out}"
    );
}

/// Declining inlining must not drop the filter targets — the preamble
/// calls them by name.
#[test]
fn declined_inlining_keeps_the_filter_target_methods() {
    let out = emitted(&app(OUT_OF_ORDER));
    assert!(out.contains("def set_room"), "set_room must survive:\n{out}");
    assert!(
        out.contains("def ensure_can_administer"),
        "ensure_can_administer must survive:\n{out}"
    );
}
