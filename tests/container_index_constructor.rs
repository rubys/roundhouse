//! `Array[...]` and `Hash[...]`: the built-in containers' own `[]`.
//!
//! A controller passing `shop: Array[String]` to `params.permit_types`
//! refused the read as `no known method [] on Array`; a model body left
//! it unchecked. Both build the container, so the read types as one.

use roundhouse::analyze::diagnose;
use std::collections::HashMap;
use std::path::PathBuf;

const CONTROLLER: &str = r#"
class ReportsController < ApplicationController
  #: -> untyped
  def raw
    shops = Array[String]
    pairs = Hash[[[1, 2]]]
    [shops.first, pairs.fetch(1), Array.nope]
  end
end
"#;

#[test]
fn array_and_hash_index_constructors_type_in_a_controller() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/reports_controller.rb", CONTROLLER),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<String> = diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    assert!(!errors.iter().any(|e| e.contains("`[]`")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("`nope`")), "{errors:?}");
}
