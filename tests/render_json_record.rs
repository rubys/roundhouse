//! `render json: <record>` for a table-backed model with no `as_json`:
//! the columns are written down as an `as_json_str` writer, and a model
//! whose columns or macros make Rails answer differently keeps the
//! runtime encoder.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :widgets do |t|\n    t.string :type\n    t.string :name\n    t.integer :count\n    t.boolean :live\n    t.date :due_on\n    t.timestamps\n  end\n  \
    create_table :prices do |t|\n    t.decimal :amount, precision: 8, scale: 2\n  end\n  \
    create_table :notes do |t|\n    t.text :data\n  end\n  \
    create_table :events do |t|\n    t.datetime :starts_at\n  end\n  \
    create_table :pairs do |t|\n    t.string :a\n    t.string :b_c\n    t.string :a_b\n    t.string :c\n  end\nend\n";

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def show
    render json: Widget.find(params[:id])
  end

  def index
    render json: Widget.all
  end

  def price
    render json: Price.find(params[:id])
  end

  def note
    render json: Note.find(params[:id])
  end

  def event
    render json: Event.find(params[:id])
  end

  def included
    render json: Widget.find(params[:id]), include: :things
  end

  def price_id
    render json: Price.find(params[:id]), only: :id
  end

  def pair_one
    render json: Pair.find(params[:id]), only: [:a_b, :c]
  end

  def pair_two
    render json: Pair.find(params[:id]), only: [:a, :b_c]
  end
end
"#;

fn emitted() -> (String, String) {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/widget.rb"), b"class Widget < ApplicationRecord\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/price.rb"), b"class Price < ApplicationRecord\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/pair.rb"), b"class Pair < ApplicationRecord\nend\n".to_vec());
    tree.insert(
        PathBuf::from("app/models/note.rb"),
        b"class Note < ApplicationRecord\n  serialize :data, coder: JSON\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/event.rb"),
        b"class Event < ApplicationRecord\n  def starts_at\n    super || Time.at(0)\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :widgets do\n    get :price, on: :member\n    get :note, on: :member\n    get :event, on: :member\n    get :included, on: :member\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/controllers/widgets_controller.rb"), CONTROLLER.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let models = ruby::emit_lowered_models(&app).into_iter().map(|f| f.content).collect::<Vec<_>>().join("\n");
    let controller = ruby::emit_lowered_controllers(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("widgets_controller"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (models, controller)
}

fn class_body<'a>(models: &'a str, name: &str) -> &'a str {
    let start = models.find(&format!("class {name} ")).unwrap_or_else(|| panic!("no class {name}:\n{models}"));
    let rest = &models[start..];
    let end = rest[1..].find("\nclass ").map(|i| i + 1).unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn a_rendered_record_gets_a_writer_over_its_columns_in_schema_order() {
    let (models, _) = emitted();
    let widget = class_body(&models, "Widget");
    assert!(widget.contains("def as_json_str"), "{widget}");
    let keys: Vec<usize> = [r#"\"id\":"#, r#",\"name\":"#, r#",\"count\":"#, r#",\"live\":"#, r#",\"due_on\":"#, r#",\"created_at\":"#, r#",\"updated_at\":"#]
        .iter()
        .map(|k| widget.find(k).unwrap_or_else(|| panic!("no {k} in\n{widget}")))
        .collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "columns out of schema order:\n{widget}");
    assert!(widget.contains("JsonBuilder.encode_datetime(self.created_at_raw)"), "{widget}");
    let writer = &widget[widget.find("def as_json_str").expect("writer")..];
    assert!(!writer.contains(r#"\"type\":"#), "the inheritance column is excepted, as Rails does:\n{writer}");
}

#[test]
fn the_render_sites_serve_the_writer_text() {
    let (_, controller) = emitted();
    let show = &controller[controller.find("def show").expect("show")..controller.find("def index").expect("index")];
    assert!(show.contains(".as_json_str") && !show.contains("JsonRender"), "{show}");
    let index = &controller[controller.find("def index").expect("index")..controller.find("def price").expect("price")];
    assert!(index.contains("as_json_str") && index.contains(".join(\",\")"), "{index}");
}

#[test]
fn a_decimal_column_or_a_serialized_attribute_keeps_the_runtime_encoder() {
    let (models, controller) = emitted();
    assert!(!class_body(&models, "Price").contains("def as_json_str\n"));
    assert!(!class_body(&models, "Note").contains("as_json_str"));
    let price = &controller[controller.find("def price").expect("price")..controller.find("def note").expect("note")];
    assert!(price.contains("JsonRender") && !price.contains("as_json_str"), "{price}");
}

#[test]
fn an_overridden_temporal_reader_keeps_the_runtime_encoder() {
    let (models, controller) = emitted();
    assert!(!class_body(&models, "Event").contains("as_json_str"));
    let event = &controller[controller.find("def event").expect("event")..controller.find("def included").expect("included")];
    assert!(event.contains("JsonRender") && !event.contains("as_json_str"), "{event}");
}


#[test]
fn an_option_the_writer_does_not_read_keeps_the_runtime_encoder() {
    let (_, controller) = emitted();
    let included = &controller[controller.find("def included").expect("included")..controller.find("def price_id").expect("price_id")];
    assert!(included.contains("JsonRender") && !included.contains("as_json_str"), "{included}");
}

#[test]
fn only_leaving_out_a_declined_column_still_gets_a_writer() {
    let (models, controller) = emitted();
    assert!(class_body(&models, "Price").contains("def as_json_str__only_2id"), "{models}");
    let price_id = &controller[controller.find("def price_id").expect("price_id")..controller.find("def pair_one").expect("pair_one")];
    assert!(price_id.contains("as_json_str__only_2id") && !price_id.contains("JsonRender"), "{price_id}");
}

#[test]
fn option_lists_that_join_alike_get_writers_of_their_own() {
    let (models, _) = emitted();
    let pair = class_body(&models, "Pair");
    assert!(pair.contains("def as_json_str__only_3a_b_1c") && pair.contains("def as_json_str__only_1a_3b_c"), "{pair}");
}
