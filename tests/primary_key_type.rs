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

// --- the write path (part 2a): a supplied key is written and answered ---

#[test]
fn a_uuid_key_is_minted_when_blank_written_and_answered_by_insert() {
    let (app, _) = app(UUID_SCHEMA, MODEL, SHOW);
    let (sig, body) = adapter_method(&app, "_adapter_insert");
    let Ty::Fn { ret, .. } = &sig else { panic!("not a signature") };
    assert_eq!(**ret, Ty::Str, "insert answers the key, not a rowid");
    assert!(body.contains("INSERT INTO widgets (id, name)"), "the key is a column of the INSERT: {body}");
    assert!(body.contains("SecureRandom.uuid"), "a blank uuid is minted: {body}");
    assert!(body.contains("@id == \"\""), "…only when blank: {body}");
    assert!(!body.contains("last_insert_rowid"), "{body}");
    assert!(body.trim_end().ends_with("@id"), "the value answered is the key ivar: {body}");
}

#[test]
fn a_string_key_is_the_apps_to_supply() {
    let schema = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", primary_key: "identifier", id: :string, force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;
    let model = "class Widget < ApplicationRecord\n  self.primary_key = \"identifier\"\nend\n";
    let (app, _) = app(schema, model, SHOW);
    let (_, body) = adapter_method(&app, "_adapter_insert");
    assert!(body.contains("INSERT INTO widgets (identifier, name)"), "{body}");
    assert!(!body.contains("SecureRandom"), "a string key is not minted: {body}");
    assert!(body.trim_end().ends_with("@identifier"), "{body}");
    // …and `id` reads the key column, so `record.id` is the key.
    let (sig, body) = adapter_method(&app, "id");
    let Ty::Fn { ret, .. } = &sig else { panic!("not a signature") };
    assert_eq!(**ret, Ty::Str);
    assert_eq!(body.trim(), "@identifier");
}

#[test]
fn an_integer_key_still_comes_back_as_the_rowid() {
    let (app, _) = app(INT_SCHEMA, MODEL, SHOW);
    let (sig, body) = adapter_method(&app, "_adapter_insert");
    let Ty::Fn { ret, .. } = &sig else { panic!("not a signature") };
    assert_eq!(**ret, Ty::Int);
    assert!(body.contains("INSERT INTO widgets (name)"), "{body}");
    assert!(body.contains("last_insert_rowid"), "{body}");
}

// --- the per-target ledger: the ruby emit carries a non-integer key, the rest say so ---

#[test]
fn a_non_integer_key_is_unsupported_per_target_not_at_ingest() {
    use roundhouse::project::{target_files, BuildTarget};
    let (app, _) = app(UUID_SCHEMA, MODEL, SHOW);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-blog");
    let unsupported = |target: BuildTarget| -> Vec<String> {
        let (_, diags) = roundhouse::emit::diagnostics::scope(|| target_files(&app, &root, target));
        diags
            .into_iter()
            .filter(|d| d.message.contains("non_integer_primary_key"))
            .map(|d| d.message)
            .collect()
    };
    assert_eq!(unsupported(BuildTarget::Ruby), Vec::<String>::new(), "the ruby emit carries the key");
    let rust = unsupported(BuildTarget::Rust);
    assert_eq!(rust.len(), 1, "{rust:?}");
    assert!(rust[0].contains("table widgets: key `id` is Uuid"), "{}", rust[0]);
    assert_eq!(unsupported(BuildTarget::Spinel), Vec::<String>::new(), "the spinel emit carries the key too");
}

/// The shipped `base.rbs` declares the key contract over `Integer`;
/// an app with a string key ships it widened to `(Integer | String)`
/// for `id`, `id=` and `_adapter_insert`, and an integer-only app
/// ships it byte-for-byte.
#[test]
fn the_spinel_sidecars_key_contract_is_as_wide_as_the_apps_keys() {
    use roundhouse::project::{target_files, BuildTarget};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-blog");
    let sidecar = |app: &roundhouse::App| -> String {
        let files = target_files(app, &root, BuildTarget::Spinel).expect("spinel files");
        files
            .into_iter()
            .find(|(p, _)| p.ends_with("runtime/active_record/base.rbs"))
            .map(|(_, c)| c)
            .expect("base.rbs in the spinel tree")
    };
    let (uuid, _) = app(UUID_SCHEMA, MODEL, SHOW);
    let wide = sidecar(&uuid);
    assert!(wide.contains("def id: () -> (Integer | String)"), "{wide}");
    assert!(wide.contains("def id=: (Integer | String) -> (Integer | String)"), "{wide}");
    assert!(wide.contains("def _adapter_insert: () -> (Integer | String)"), "{wide}");
    let (int, _) = app(INT_SCHEMA, MODEL, SHOW);
    let narrow = sidecar(&int);
    assert!(narrow.contains("def id: () -> Integer\n"), "{narrow}");
    assert!(narrow.contains("def _adapter_insert: () -> Integer\n"), "{narrow}");
    assert_eq!(
        narrow,
        std::fs::read_to_string("runtime/ruby/active_record/base.rbs").expect("source sidecar"),
    );
}


#[test]
fn a_uuid_foreign_key_uses_a_string_sentinel() {
    let schema = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", id: :uuid, force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "parts", force: :cascade do |t|
    t.uuid "widget_id", null: false
  end
end
"#;
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), schema.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/widget.rb"), b"class Widget < ApplicationRecord\n  has_many :parts\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/part.rb"), b"class Part < ApplicationRecord\n  belongs_to :widget\nend\n".to_vec());
    let app = ingest_app_from_tree(tree).expect("ingest");
    let part = app.models.iter().find(|m| m.name.0.as_str() == "Part").expect("Part");
    let lc = roundhouse::lower::model_to_library::lower_model_to_library_class(part, &app.schema);
    let body = |name: &str| {
        roundhouse::emit::ruby::emit_expr(
            &lc.methods.iter().find(|m| m.name.as_str() == name).unwrap_or_else(|| panic!("no {name}")).body,
        )
    };
    assert!(body("widget").contains("@widget_id == \"\""), "reader: {}", body("widget"));
    assert!(body("widget=").contains("@widget_id = \"\""), "writer: {}", body("widget="));
    assert!(!body("widget=").contains("= 0"), "writer must not reset a uuid to 0: {}", body("widget="));
}

/// The base's `id`/`id=` are raise-bodied — a contract, no slot — and
/// on the transpile path a raise-bodied reader/writer pair reads as the
/// attribute its subclasses override (`runtime_src::
/// reclassify_abstract_attributes`), so a property-typed target renders
/// `var id`, not a function pair no property can override.
#[test]
fn the_base_key_contract_transpiles_as_an_attribute() {
    use roundhouse::dialect::AccessorKind;
    let rb = std::fs::read_to_string("runtime/ruby/active_record/base.rb").expect("base.rb");
    let rbs = std::fs::read_to_string("runtime/ruby/active_record/base.rbs").expect("base.rbs");
    let classes = roundhouse::runtime_src::parse_library_with_rbs(
        rb.as_bytes(),
        &rbs,
        "runtime/ruby/active_record/base.rb",
    )
    .expect("parse base");
    let base = classes.iter().find(|c| c.name.0.as_str() == "ActiveRecord::Base").expect("Base");
    let kind = |name: &str| base.methods.iter().find(|m| m.name.as_str() == name).map(|m| m.kind);
    assert_eq!(kind("id"), Some(AccessorKind::AttributeReader));
    assert_eq!(kind("id="), Some(AccessorKind::AttributeWriter));
    // …and the source really has no slot: no `@id` outside comments.
    let slot_reads = rb
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter(|l| l.contains("@id ") || l.contains("@id\n") || l.ends_with("@id") || l.contains("@id="))
        .count();
    assert_eq!(slot_reads, 0, "the base must not touch @id");
}
