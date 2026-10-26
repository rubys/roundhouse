//! An association's namespaced `class_name:` reads only the class it names.
//!
//! `belongs_to :record, class_name: "Vendor::Record"` names a class that
//! a gem may define and the app does not. The relation lowering resolved
//! the receiver `Vendor::Record` by its last segment when the full name
//! was not registered, so an app that also has a top-level `Record`
//! read `records` through `Record` instead. Rails never does that: the
//! reader loads `Vendor::Record` or raises `NameError` ("Missing model
//! class Vendor::Record for the Widget#record association").
//!
//! A namespaced class the app does define keeps reading its own table.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  \
    create_table \"records\", force: :cascade do |t|\n    t.string \"title\"\n  end\n  \
    create_table \"gadgets\", force: :cascade do |t|\n    t.string \"label\"\n    t.integer \"widget_id\"\n  end\n  \
    create_table \"vendor_records\", force: :cascade do |t|\n    t.string \"uid\"\n  end\n  \
    create_table \"widgets\", force: :cascade do |t|\n    t.integer \"record_id\", null: false\n  end\n\
    end\n";
const WIDGET: &str = "class Widget < ApplicationRecord\n  \
    belongs_to :record, class_name: \"Vendor::Record\"\n  \
    has_one :gadget, class_name: \"Vendor::Gadget\"\n\
    end\n";

fn widget_rb(extra: &[(&str, &str)]) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, content) in [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/record.rb", "class Record < ApplicationRecord\nend\n"),
        ("app/models/gadget.rb", "class Gadget < ApplicationRecord\nend\n"),
        ("app/models/widget.rb", WIDGET),
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .chain(extra.iter().copied())
    {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files");
    files
        .into_iter()
        .find(|(p, _)| p == "app/models/widget.rb")
        .map(|(_, body)| body)
        .expect("app/models/widget.rb not emitted")
}

/// The body of `def <name>` up to the next `def`.
fn method<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("  def {name}\n"))
        .unwrap_or_else(|| panic!("no `def {name}` in:\n{src}"));
    let rest = &src[start + 1..];
    let end = rest.find("\n  def ").map_or(rest.len(), |i| i + 1);
    &rest[..end]
}

#[test]
fn an_unmodeled_namespaced_class_name_does_not_read_the_top_level_class() {
    let src = widget_rb(&[]);
    let record = method(&src, "record");
    assert!(!record.contains("FROM records"), "{record}");
    assert!(!record.contains(" Record.from_stmt"), "{record}");
    assert!(record.contains("Vendor::Record"), "{record}");
    let gadget = method(&src, "gadget");
    assert!(!gadget.contains("FROM gadgets"), "{gadget}");
    assert!(!gadget.contains(" Gadget.from_stmt"), "{gadget}");
    assert!(gadget.contains("Vendor::Gadget"), "{gadget}");
}

#[test]
fn a_modeled_namespaced_class_name_reads_its_own_table() {
    let src = widget_rb(&[
        (
            "app/models/vendor/record.rb",
            "module Vendor\n  class Record < ApplicationRecord\n    self.table_name = \"vendor_records\"\n  end\nend\n",
        ),
        (
            "app/models/vendor/gadget.rb",
            "module Vendor\n  class Gadget < ApplicationRecord\n    self.table_name = \"gadgets\"\n  end\nend\n",
        ),
    ]);
    let record = method(&src, "record");
    assert!(record.contains("FROM vendor_records"), "{record}");
    assert!(record.contains("Vendor::Record.from_stmt"), "{record}");
    let gadget = method(&src, "gadget");
    assert!(gadget.contains("Vendor::Gadget.from_stmt"), "{gadget}");
}
