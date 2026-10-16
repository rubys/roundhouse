//! A model's `normalizes` declarations emit their synthesized
//! `_normalize_<attr>` class methods in declaration order, in both the
//! `.rb` and the `.rbs` (#732).
//!
//! `normalizations(model)` collected the attribute -> (param, body) map
//! in a `HashMap`, and `push_normalize_methods` walked that map to push
//! one method per attribute — so the emitted order followed the map's
//! hash order (which varies run to run, since `HashMap` is randomly
//! seeded) instead of the order the attributes were declared in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const WIDGETS_CONTROLLER: &str =
    "class WidgetsController < ApplicationController\n  def show\n    render json: { ok: 1 }\n  end\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"slug\"\n    t.string \"name\"\n    t.string \"label\"\n    t.string \"code\"\n  end\nend\n";
const ROUTES: &str = "Rails.application.routes.draw do\n  root \"widgets#show\"\nend\n";

/// Four `normalizes` declarations, deliberately out of alphabetical
/// order, each with a distinct one-line `with:` lambda so the attrs
/// can't be told apart by coincidence.
const WIDGET: &str = r#"class Widget < ApplicationRecord
  normalizes :slug, with: ->(v) { v.strip }
  normalizes :name, with: ->(v) { v.downcase }
  normalizes :label, with: ->(v) { v.upcase }
  normalizes :code, with: ->(v) { v.to_s }
end
"#;

/// `app/models/widget.rb` and `app/models/widget.rbs` for the fixture
/// above.
fn widget_files() -> (String, String) {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, content) in [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER),
        ("app/models/widget.rb", WIDGET),
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
    ] {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files");
    let rb = files
        .iter()
        .find(|(p, _)| p == "app/models/widget.rb")
        .map(|(_, c)| c.clone())
        .expect("widget.rb emitted");
    let rbs = files
        .iter()
        .find(|(p, _)| p == "app/models/widget.rbs")
        .map(|(_, c)| c.clone())
        .expect("widget.rbs emitted");
    (rb, rbs)
}

/// The `_normalize_<attr>` names, in the order their definitions appear
/// in `text` (`def self._normalize_x` in a `.rb`, `def self._normalize_x:
/// ...` in a `.rbs`).
fn normalize_order(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("def self._normalize_"))
        .map(|rest| rest.split(|c: char| c == '(' || c == ':').next().unwrap_or(rest).trim())
        .collect()
}

const EXPECTED: &[&str] = &["slug", "name", "label", "code"];

#[test]
fn normalize_methods_emit_in_declaration_order() {
    let (rb, rbs) = widget_files();
    let rb_order = normalize_order(&rb);
    let rbs_order = normalize_order(&rbs);
    assert_eq!(rb_order, EXPECTED, "app/models/widget.rb:\n{rb}");
    assert_eq!(rbs_order, EXPECTED, "app/models/widget.rbs:\n{rbs}");
}

#[test]
fn normalize_methods_emit_identically_every_time() {
    // Each analysis builds fresh `HashMap`s with fresh random seeds, so
    // repeated emits walked the attributes in varying orders before the
    // fix.
    let (first_rb, first_rbs) = widget_files();
    for _ in 0..12 {
        let (rb, rbs) = widget_files();
        assert_eq!(rb, first_rb);
        assert_eq!(rbs, first_rbs);
    }
}
