//! `render json: <PORO>` — a plain object has no `as_json`, so the
//! encoder falls through to `to_s` and the response body is
//! `"#<Opengraph::Metadata:0x0000000121772a50>"`.
//!
//! Rails does not have this problem because ActiveSupport puts `as_json`
//! on `Object` itself, answering `instance_values` — reflection the
//! shared runtime cannot have. The compiler knows the answer, so
//! `lower::as_json_poro` writes it down for the classes an app actually
//! renders as JSON.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

/// A tableless class under `app/models/` (which is where a Rails app
/// puts an `ActiveModel::Model`), plus a controller that renders it.
fn emit_with(controller: &str, model: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/card.rb"), model.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/rooms_controller.rb"),
        controller.as_bytes().to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("card"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The same tree, the CONTROLLER's emit — where the `render json:` site
/// itself lands.
fn controller_emit_with(controller: &str, model: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/card.rb"), model.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/rooms_controller.rb"),
        controller.as_bytes().to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("rooms_controller"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

const RENDERS_JSON: &str = r#"class RoomsController < ApplicationController
  def show
    card = Card.new
    render json: card
  end
end
"#;

const RENDERS_NOTHING: &str = r#"class RoomsController < ApplicationController
  def show
    @room = Room.first
  end
end
"#;

/// The SPLAT form, which is how campfire's `Opengraph::Metadata` names
/// its fields — and the reason this reuses `declared_attr_names` rather
/// than re-deriving the list.
const SPLAT_MODEL: &str = r#"class Card
  include ActiveModel::Model

  ATTRIBUTES = %i[ title url ]
  attr_accessor *ATTRIBUTES
end
"#;

#[test]
fn a_json_rendered_poro_gets_an_as_json_over_its_declared_attributes() {
    let src = emit_with(RENDERS_JSON, SPLAT_MODEL);
    assert!(
        src.contains(r#"{ "title" => @title, "url" => @url }"#),
        "as_json must read the declared attributes, STRING-keyed as \
         `instance_values` is:\n{src}"
    );
    assert!(src.contains("def as_json(options = {})"), "{src}");
}

/// DEMAND-GATED, the way `to_attrs` is: a PORO nobody renders as JSON
/// carries no `as_json`. A method on every plain class in the app would
/// be dead weight, and on a strict target dead weight that type-checks.
#[test]
fn a_poro_nobody_renders_as_json_gets_nothing() {
    let src = emit_with(RENDERS_NOTHING, SPLAT_MODEL);
    assert!(!src.contains("as_json"), "{src}");
}

/// A hand-written `as_json` wins, exactly as it does in Rails.
#[test]
fn a_declared_as_json_is_left_alone() {
    let src = emit_with(
        RENDERS_JSON,
        r#"class Card
  include ActiveModel::Model

  attr_accessor :title, :url

  def as_json(options = {})
    { "t" => title }
  end
end
"#,
    );
    assert!(src.contains(r#"{ "t" => title }"#), "{src}");
    assert!(
        !src.contains(r#""title" => @title"#),
        "the synthesized body must not be added beside the app's own:\n{src}"
    );
}

/// The encoder is monomorphized beside the Hash: the same readers become
/// an `as_json_str` writer in the jbuilder accumulator shape, so a strict
/// target has text to serve without the CRuby overlay's reflective
/// `JsonRender.encode`.
#[test]
fn a_json_rendered_poro_gets_a_writer_over_the_same_readers() {
    let src = emit_with(RENDERS_JSON, SPLAT_MODEL);
    assert!(src.contains("def as_json_str"), "{src}");
    for line in [
        r#"io << "{""#,
        r#"io << "\"title\":""#,
        "io << JsonBuilder.encode_value(self.title)",
        r#"io << ",\"url\":""#,
        "io << JsonBuilder.encode_value(self.url)",
        r#"io << "}""#,
    ] {
        assert!(src.contains(line), "writer must append {line:?}:\n{src}");
    }
}

/// The call site becomes the Rails idiom for an already-encoded body —
/// `render plain: <v>.as_json_str, content_type: "application/json"` —
/// which the controller rewrite lowers on every target. No
/// `JsonRender.encode` is left for a compiled tree to fail to resolve.
#[test]
fn the_render_site_serves_the_writers_text_as_json() {
    let src = controller_emit_with(RENDERS_JSON, SPLAT_MODEL);
    assert!(
        src.contains(r#"render(card.as_json_str, content_type: "application/json")"#),
        "{src}"
    );
    assert!(!src.contains("JsonRender"), "{src}");
}

/// An author's own `content_type:` stands, as it does for `plain:`.
#[test]
fn an_explicit_content_type_on_the_render_site_stands() {
    let src = controller_emit_with(
        r#"class RoomsController < ApplicationController
  def show
    card = Card.new
    render json: card, content_type: "application/vnd.api+json", status: :created
  end
end
"#,
        SPLAT_MODEL,
    );
    assert!(
        src.contains(r#"render(card.as_json_str, content_type: "application/vnd.api+json", status: :created)"#),
        "{src}"
    );
}

/// A class with its own `as_json` gets no writer (its pairs are not
/// recognized here), so its site keeps the runtime encoder — CRuby-only,
/// loud elsewhere, and never wrong text.
#[test]
fn a_declared_as_json_keeps_the_runtime_encoder_at_the_site() {
    let src = controller_emit_with(
        RENDERS_JSON,
        r#"class Card
  include ActiveModel::Model

  attr_accessor :title, :url

  def as_json(options = {})
    { "t" => title }
  end
end
"#,
    );
    assert!(src.contains("ActionController::JsonRender.encode(card)"), "{src}");
    assert!(!src.contains("as_json_str"), "{src}");
}
