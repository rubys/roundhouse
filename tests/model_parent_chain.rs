//! A model descending through the app's own abstract base is a model.
//!
//! `classify_class_file` matched a superclass against two literals —
//! `ApplicationRecord` and `ActiveRecord::Base`. A class one step
//! further down was ingested as a plain library class, so its
//! associations, validations and scopes became unknown class-body
//! calls: replayed by the ruby emitter, dropped by a strict target.
//!
//! The shape is not exotic. Rails' own multiple-database guide
//! prescribes an abstract base per database, and an engine or a
//! packwerk package gets one by default.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n    t.integer \"reading_id\"\n  end\n  create_table \"readings\", force: :cascade do |t|\n    t.string \"note\", null: false\n  end\nend\n";

fn emitted(files: &[(&str, &str)], want: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    for (p, c) in files {
        tree.insert(PathBuf::from(*p), c.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .chain(ruby::emit_lowered_models(&app))
        .find(|f| f.path.display().to_string().ends_with(want))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("{want} is emitted"))
}

const APP_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const PKG_RECORD: &str = "class PkgRecord < ApplicationRecord\n  self.abstract_class = true\nend\n";

#[test]
fn an_association_survives_one_abstract_base() {
    let emitted = emitted(
        &[
            ("app/models/application_record.rb", APP_RECORD),
            ("app/models/pkg_record.rb", PKG_RECORD),
            ("app/models/reading.rb", "class Reading < PkgRecord\nend\n"),
            (
                "app/models/gauge.rb",
                "class Gauge < PkgRecord\n  belongs_to :reading\nend\n",
            ),
        ],
        "gauge.rb",
    );
    assert!(emitted.contains("def reading"), "got:\n{emitted}");
    // And the columns, which a library class never gets either.
    assert!(emitted.contains("def label"), "got:\n{emitted}");
}

#[test]
fn the_chain_closes_regardless_of_file_order() {
    // The base is read AFTER its user here. A recursive resolver that
    // trusted file order would classify `Gauge` before knowing what
    // `PkgRecord` is — which is why the pre-pass iterates to a
    // fixpoint instead.
    let emitted = emitted(
        &[
            ("app/models/zz_application_record.rb", APP_RECORD),
            ("app/models/zz_pkg_record.rb", PKG_RECORD),
            ("app/models/aaa_gauge.rb", "class Gauge < PkgRecord\n  belongs_to :reading\nend\n"),
            ("app/models/reading.rb", "class Reading < PkgRecord\nend\n"),
        ],
        "gauge.rb",
    );
    assert!(emitted.contains("def reading"), "got:\n{emitted}");
}

#[test]
fn two_abstract_bases_deep_still_resolves() {
    let emitted = emitted(
        &[
            ("app/models/application_record.rb", APP_RECORD),
            ("app/models/pkg_record.rb", PKG_RECORD),
            (
                "app/models/inner_record.rb",
                "class InnerRecord < PkgRecord\n  self.abstract_class = true\nend\n",
            ),
            ("app/models/reading.rb", "class Reading < InnerRecord\nend\n"),
            (
                "app/models/gauge.rb",
                "class Gauge < InnerRecord\n  belongs_to :reading\nend\n",
            ),
        ],
        "gauge.rb",
    );
    assert!(emitted.contains("def reading"), "got:\n{emitted}");
}

#[test]
fn a_plain_class_under_app_models_is_still_a_library_class() {
    // The guard the two literals were there for: a PORO under
    // app/models must not become a model because the chain walk got
    // enthusiastic.
    let emitted = emitted(
        &[
            ("app/models/application_record.rb", APP_RECORD),
            ("app/models/calculator.rb", "class Calculator\n  def add\n    1\n  end\nend\n"),
        ],
        "calculator.rb",
    );
    assert!(emitted.contains("def add"), "got:\n{emitted}");
    assert!(!emitted.contains("def save"), "got:\n{emitted}");
}

#[test]
fn a_concrete_parent_means_sti_and_is_left_alone() {
    // The distinction the first version of this got wrong. Closing
    // over every model reclassified a single-table-inheritance
    // subclass as a model — which it is in Rails, but not in this
    // ingest, where STI is handled through the library-class path.
    //
    // A concrete parent with a table of its own means STI. An
    // ABSTRACT one means a base chain. Only the latter joins the set.
    let emitted = emitted(
        &[
            ("app/models/application_record.rb", APP_RECORD),
            ("app/models/reading.rb", "class Reading < ApplicationRecord\nend\n"),
            ("app/models/readings/special.rb", "class Readings::Special < Reading\nend\n"),
        ],
        "special.rb",
    );
    // A model would have been given the table's columns; the STI
    // subclass is not, because it is not one here.
    assert!(!emitted.contains("def note"), "got:\n{emitted}");
}
