//! `module Catalog; class Item; class Detail < ApplicationRecord`
//! — a model nested in a class its file reopens only as a namespace.
//!
//! A catalog/item/detail.rb is written so. Model ingest and
//! the file classification took the first class in the file, `Item`
//! (no superclass): the file became a library class, `Detail` a
//! plain nested class with no table, and every column read on it
//! failed.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"
ActiveRecord::Schema.define(version: 1) do
  create_table "catalog_items", force: :cascade do |t|
    t.string "name"
  end
  create_table "catalog_item_details", force: :cascade do |t|
    t.integer "catalog_item_id"
    t.string "dimension_unit"
  end
end
"#;

const ITEM: &str = r#"
module Catalog
  class Item < ApplicationRecord
    self.table_name = "catalog_items"
  end
end
"#;

const DETAIL: &str = r#"
module Catalog
  class Item
    class Detail < ApplicationRecord
      self.table_name = "catalog_item_details"
      belongs_to :catalog_item, class_name: "::Catalog::Item"

      #: -> untyped
      def unit = dimension_unit.yyy
    end
  end
end
"#;

fn app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/catalog/item.rb", ITEM),
        ("app/models/catalog/item/detail.rb", DETAIL),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn errors(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error"))
        .collect()
}

/// The nested class is the model: it binds its table, so a column
/// reads its type, and it emits a row class.
#[test]
fn the_nested_class_is_the_model_and_binds_its_table() {
    let app = app();
    assert!(app.models.iter().any(|m| m.name.0.as_str() == "Catalog::Item::Detail"));
    let errors = errors(&app);
    assert!(errors.iter().any(|e| e.contains("no known method `yyy` on String")), "{errors:?}");
    assert!(!errors.iter().any(|e| e.contains("`dimension_unit`")), "{errors:?}");
    let files = ruby::emit_spinel(&app);
    assert!(
        files.iter().any(|f| f.path.to_string_lossy().ends_with("catalog/item/detail_row.rb")),
        "no row class for the nested model"
    );
}
