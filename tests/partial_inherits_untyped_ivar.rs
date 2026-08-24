//! A partial that reads its renderer's IVAR must be bound to it, even
//! when the renderer's own binding is `Ty::Var`.
//!
//! The propagation dropped noise (`Var` / `Untyped`) before merging,
//! which is right for a UNION — a shapeless variant only pollutes one.
//! It is wrong when the partial has no entry at all, because the two
//! outcomes are not symmetric: an unknown type is NOT a diagnostic, and
//! an ABSENT binding is. So the renderer typed clean and the partial it
//! rendered reported `@room has no known type` for the same ivar.
//!
//! campfire's `rooms/show` is the shape: `@room` is seeded through a
//! `before_action` whose chain bottoms out in an unmodeled
//! `CurrentAttributes` read, so it lands as `Var`; `rooms/show.html.erb`
//! reads it without complaint, and `rooms/show/_invitation` — which
//! reaches for the ivar rather than the `room:` local it is handed —
//! was three errors.

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

// `@room` comes from a helper whose receiver is unmodeled, so the
// binding exists with no shape — the `Var` case, not the absent one.
const CONTROLLER: &str = r#"
class RoomsController < ApplicationController
  before_action :set_room

  def show
  end

  private
    def set_room
      @room = Unmodeled::Thing.current.rooms.first
    end
end
"#;

// The renderer passes a local; the partial reads the IVAR anyway, which
// Rails allows and campfire does.
const SHOW: &str = "<h1>room</h1>\n<%= render \"rooms/show/detail\", room: @room %>\n";
const PARTIAL: &str = "<p><%= @room %></p>\n";

#[test]
fn a_partial_inherits_a_var_typed_ivar_from_its_renderer() {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/rooms_controller.rb", CONTROLLER),
        ("app/views/rooms/show.html.erb", SHOW),
        ("app/views/rooms/show/_detail.html.erb", PARTIAL),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);

    // Asserted on the BINDING, not on `diagnose`: what went wrong is
    // that the partial had no entry for an ivar its renderer did, and
    // that is visible here directly. Reading it off the diagnostics
    // instead would depend on how a `Var` binding is reported, which is
    // a separate question from whether the binding arrived.
    let ivars_for = |name: &str| -> Vec<String> {
        app.view_ivar_types
            .get(&roundhouse::Symbol::from(name))
            .map(|m| m.keys().map(|k| k.as_str().to_string()).collect())
            .unwrap_or_default()
    };
    assert!(
        ivars_for("rooms/show").contains(&"room".to_string()),
        "precondition: the renderer must bind @room — {:?}",
        ivars_for("rooms/show")
    );
    assert!(
        ivars_for("rooms/show/_detail").contains(&"room".to_string()),
        "the partial must inherit its renderer's @room binding, shapeless or not — {:?}",
        ivars_for("rooms/show/_detail")
    );
}
