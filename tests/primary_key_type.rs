//! The typed `id` surface follows the schema's primary key (#90).
//!
//! `ApplicationRecord` declared `id`, `id=`, `find`, `exists?` and the
//! `_adapter_*` key primitives as `Ty::Int` for every model, whatever
//! the schema said — so on a uuid-keyed app `Thing.find(params[:id])`
//! took an Integer it was never given, and `thing.id.upcase` was a
//! dispatch error on a String the schema declared. The key column's
//! type now decides, per model; the RUNTIME write path (insert's
//! `last_insert_rowid`) is the half that stays ledgered.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, Severity};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;
use roundhouse::Symbol;

fn app(schema: &str, model: &str, controller: &str) -> (roundhouse::App, Analyzer) {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), schema.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/widget.rb"), model.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/controllers/widgets_controller.rb"),
        controller.as_bytes().to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :widgets, only: [:show]\nend\n".to_vec(),
    );
    // The non-integer key is a ledger line (the runtime half), which
    // is an ingest error outside survey mode.
    roundhouse::ingest::survey::activate();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _gaps = roundhouse::ingest::survey::drain();
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    (app, analyzer)
}

const UUID_SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", id: :uuid, default: -> { "gen_random_uuid()" }, force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

const INT_SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

const MODEL: &str = "class Widget < ApplicationRecord\nend\n";

/// `find` takes the key as a String and `id` answers one: a String
/// method on it resolves, and the whole action types clean.
const SHOW: &str = r#"class WidgetsController < ApplicationController
  def show
    @widget = Widget.find(params[:id])
    @label = @widget.id.upcase
  end
end
"#;

fn errors(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect()
}

/// What the analyzer's registry answers for `Class#method` (a return
/// type; the analyzer keeps no parameter types for framework methods).
fn registry_ty(analyzer: &Analyzer, class: &str, method: &str, class_side: bool) -> Ty {
    let info = analyzer
        .class_registry()
        .get(&roundhouse::ClassId(Symbol::from(class)))
        .unwrap_or_else(|| panic!("no registry entry for {class}"));
    let table = if class_side { &info.class_methods } else { &info.instance_methods };
    table
        .get(&Symbol::from(method))
        .cloned()
        .unwrap_or_else(|| panic!("{class}#{method} not declared"))
}

/// The synthesized adapter primitive's signature and rendered body —
/// the emitted `find`/`exists?` surface every lane compiles.
fn adapter_method(app: &roundhouse::App, name: &str) -> (Ty, String) {
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Widget").expect("Widget");
    let lc = roundhouse::lower::model_to_library::lower_model_to_library_class(model, &app.schema);
    let m = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == name)
        .unwrap_or_else(|| panic!("no {name} on Widget"));
    (m.signature.clone().expect("signature"), roundhouse::emit::ruby::emit_expr(&m.body))
}

fn param_ty(sig: &Ty, i: usize) -> Ty {
    let Ty::Fn { params, .. } = sig else { panic!("not a signature: {sig:?}") };
    params[i].ty.clone()
}

#[test]
fn a_uuid_key_types_id_and_find_as_strings() {
    let (app, analyzer) = app(UUID_SCHEMA, MODEL, SHOW);
    assert_eq!(registry_ty(&analyzer, "Widget", "id", false), Ty::Str, "id answers the key");
    assert_eq!(
        registry_ty(&analyzer, "Widget", "ids", true),
        Ty::Array { elem: Box::new(Ty::Str) },
        "ids projects the key"
    );
    assert_eq!(errors(&app), Vec::<String>::new());

    // The emitted finders take the key as a String and bind it as one.
    let (sig, body) = adapter_method(&app, "_adapter_find_by_id");
    assert_eq!(param_ty(&sig, 0), Ty::Str);
    assert!(body.contains("WHERE id = "), "{body}");
    assert!(
        body.contains("escape_string(id)") || body.contains("bind_text("),
        "a uuid key must not be bound as an integer: {body}"
    );
    let (sig, _) = adapter_method(&app, "_adapter_exists_by_id?");
    assert_eq!(param_ty(&sig, 0), Ty::Str);
}

#[test]
fn the_default_key_is_still_an_integer() {
    let (app, analyzer) = app(INT_SCHEMA, MODEL, SHOW);
    assert_eq!(registry_ty(&analyzer, "Widget", "id", false), Ty::Int);
    let (sig, body) = adapter_method(&app, "_adapter_find_by_id");
    assert_eq!(param_ty(&sig, 0), Ty::Int);
    assert!(body.contains("escape_int(id)") || body.contains("bind_int("), "{body}");
    // …and `upcase` on it is the dispatch error it should be.
    let errs = errors(&app);
    assert!(
        errs.iter().any(|e| e.contains("upcase")),
        "expected a dispatch error on Integer#upcase, got {errs:?}"
    );
}

/// `create_table primary_key: "identifier", id: :string`: the key is a
/// column that is not called `id`. `record.id` still answers it, and
/// the emitted finders compare THAT column.
#[test]
fn a_named_primary_key_is_the_declared_column() {
    let schema = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", primary_key: "identifier", id: :string, force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;
    let model = "class Widget < ApplicationRecord\n  self.primary_key = \"identifier\"\nend\n";
    let (app, analyzer) = app(schema, model, SHOW);
    assert_eq!(registry_ty(&analyzer, "Widget", "id", false), Ty::Str);
    assert_eq!(errors(&app), Vec::<String>::new());
    let (sig, body) = adapter_method(&app, "_adapter_find_by_id");
    assert_eq!(param_ty(&sig, 0), Ty::Str);
    assert!(body.contains("WHERE identifier = "), "{body}");
    let (_, body) = adapter_method(&app, "_adapter_delete");
    assert!(body.contains("WHERE identifier = ") && body.contains("@identifier"), "{body}");
}
