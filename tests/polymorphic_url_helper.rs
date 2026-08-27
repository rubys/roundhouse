//! A bare `polymorphic_url` in a helper body resolves to the runtime's
//! `ActionView::ViewHelpers` member.
//!
//! campfire's `BroadcastsHelper.broadcast_image_path` asks Rails for
//! "the route for whatever this record is" and hands it an Active
//! Storage representation. The helper qualification knew `image_tag`
//! and `image_path` on the lines around it and not this one, so the
//! call emitted bare — a method NOTHING defines, which is a NameError
//! on CRuby and stops a strict build outright.
//!
//! The runtime member RAISES (a record's route is resolved at transpile
//! time; the Active Storage bytes half is unmodeled either way). That
//! is the point: the gap gets one named home instead of reading like a
//! compiler bug.

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

#[test]
fn a_bare_polymorphic_url_gets_its_receiver() {
    let tree: HashMap<PathBuf, Vec<u8>> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/helpers/broadcasts_helper.rb",
            "module BroadcastsHelper\n  def broadcast_image_path(image)\n    \
             if image.is_a?(String)\n      image_path(image)\n    else\n      \
             polymorphic_url(image, only_path: true)\n    end\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("broadcasts_helper.rb"))
        .map(|f| f.content.clone())
        .expect("broadcasts_helper.rb");
    assert!(
        src.contains("ActionView::ViewHelpers.polymorphic_url("),
        "the bare call must resolve where the runtime defines it, beside \
         the `image_path` on the branch above it:\n{src}"
    );
}

/// `asset_path` is the same gap one line over: campfire's
/// `message_sound_presentation` asks the GENERAL asset helper for an
/// mp3, and the qualification knew `image_path` and not this one.
#[test]
fn a_bare_asset_path_gets_its_receiver() {
    let tree: HashMap<PathBuf, Vec<u8>> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/helpers/sounds_helper.rb",
            "module SoundsHelper\n  def sound_url(name)\n    \
             asset_path(\"#{name}.mp3\")\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("sounds_helper.rb"))
        .map(|f| f.content.clone())
        .expect("sounds_helper.rb");
    assert!(
        src.contains("ActionView::ViewHelpers.asset_path("),
        "the bare asset helper must resolve where the runtime defines it:\n{src}"
    );
}
