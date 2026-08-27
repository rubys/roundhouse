//! A controller concern's own copy of its instance methods, after the
//! splice, has no caller and no includer — and it does not emit.
//!
//! `splice_concerns_into_controllers` copies the methods into each
//! including controller and drops the `include`. The module still
//! emitted its ORIGINAL bodies, which are controller-context code:
//! campfire's `Authentication#request_authentication` calls
//! `redirect_to`, `Authorization#ensure_can_administer` calls `head`,
//! and neither name exists inside a module nothing includes. Spinel
//! resolves a module's receiverless sends through its includers, so
//! the husk stopped the campfire build one concern at a time.
//!
//! The second test is the reason the prune is gated on more than "was
//! spliced": a concern included by something that is NOT a controller
//! keeps every method, because that includer still calls them.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

const AUTHORIZATION: &str = r#"module Authorization
  extend ActiveSupport::Concern

  private
    def ensure_can_administer
      head :forbidden unless Current.user.can_administer?
    end
end
"#;

fn app(extra: &[(&str, &str)]) -> roundhouse::App {
    let mut files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/controllers/concerns/authorization.rb", AUTHORIZATION),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/rooms_controller.rb",
            "class RoomsController < ApplicationController\n  include Authorization\n\n  \
             before_action :ensure_can_administer\n\n  def show\n    \
             @room = Room.find(params[:id])\n  end\nend\n",
        ),
        ("app/views/rooms/show.html.erb", "<p><%= @room.name %></p>\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ];
    files.extend_from_slice(extra);
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    // The prune is a POST-ANALYZE lowering: a test that emits straight
    // off the ingest never runs it and passes for the wrong reason.
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    app
}

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
fn the_spliced_copy_is_where_the_method_lives() {
    let app = app(&[]);
    let controller = emitted(&app, "rooms_controller.rb");
    // The filter inlining runs the spliced body straight into the
    // action, so what proves the splice is the BODY, not the `def`.
    assert!(
        controller.contains("head(:forbidden)"),
        "the splice puts the concern's body on the controller:\n{controller}"
    );
    let module = emitted(&app, "authorization.rb");
    assert!(
        !module.contains("def ensure_can_administer"),
        "the module's own copy has no caller and no includer — it must not \
         emit a body full of controller sends:\n{module}"
    );
    assert!(
        module.contains("module Authorization"),
        "the module itself stays (it is required, and it is where its \
         constants live):\n{module}"
    );
}

#[test]
fn a_non_controller_includer_keeps_the_methods() {
    // `ApplicationCable::Connection` includes campfire's
    // `Authentication::SessionLookup` and calls into it. Same shape: a
    // plain class that is not a controller, so nothing spliced anything
    // for it and the module is the only definition there is.
    let app = app(&[(
        "app/models/audit.rb",
        "class Audit\n  include Authorization\nend\n",
    )]);
    let module = emitted(&app, "authorization.rb");
    assert!(
        module.contains("def ensure_can_administer"),
        "a live `include` outside a controller keeps the module's \
         methods:\n{module}"
    );
}
