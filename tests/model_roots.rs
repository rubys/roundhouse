//! An ActiveRecord class is a model wherever it lives.
//!
//! The models directory was `app/models` alone, so an app keeping its
//! ActiveRecord classes in a package under `lib/` — or anywhere it
//! adds to `config.autoload_paths` — had them ingested as plain
//! library classes. Associations, validations and scopes became
//! unknown class-body calls: replayed by the ruby emitter, dropped by
//! a strict target.
//!
//! `support_roots` already collected those directories and their files
//! were already read; only the classification never ran on them.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::Diagnostic;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n    t.integer \"reading_id\"\n  end\n  create_table \"readings\", force: :cascade do |t|\n    t.string \"note\", null: false\n  end\nend\n";
const APP_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

fn emitted(files: &[(&str, &str)], want: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APP_RECORD),
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

#[test]
fn a_model_under_lib_is_a_model() {
    let emitted = emitted(
        &[
            ("lib/pkg/reading.rb", "class Reading < ApplicationRecord\nend\n"),
            (
                "lib/pkg/gauge.rb",
                "class Gauge < ApplicationRecord\n  belongs_to :reading\nend\n",
            ),
        ],
        "gauge.rb",
    );
    assert!(emitted.contains("def reading"), "got:\n{emitted}");
    assert!(emitted.contains("def label"), "got:\n{emitted}");
}

#[test]
fn the_base_chain_resolves_across_both_trees() {
    // The abstract base lives under `lib/` and the model under
    // `app/models`, so neither tree alone can classify it. The
    // pre-pass covers both before either is walked.
    let emitted = emitted(
        &[
            (
                "lib/pkg/pkg_record.rb",
                "class PkgRecord < ApplicationRecord\n  self.abstract_class = true\nend\n",
            ),
            ("app/models/reading.rb", "class Reading < PkgRecord\nend\n"),
            (
                "app/models/gauge.rb",
                "class Gauge < PkgRecord\n  belongs_to :reading\nend\n",
            ),
        ],
        "gauge.rb",
    );
    assert!(emitted.contains("def reading"), "got:\n{emitted}");
}

#[test]
fn a_plain_class_under_lib_stays_a_library_class() {
    // The guard. `lib/` is full of things that are not models, and
    // only an explicit Model classification routes that way — unlike
    // under `app/models`, where the directory is the app saying so.
    let emitted = emitted(
        &[(
            "lib/pkg/calculator.rb",
            "class Calculator\n  def add\n    1\n  end\nend\n",
        )],
        "calculator.rb",
    );
    assert!(emitted.contains("def add"), "got:\n{emitted}");
    assert!(!emitted.contains("def save"), "got:\n{emitted}");
}

/// Every diagnostic the post-analyze pipeline produced for `files`.
fn diagnostics(files: &[(&str, &str)]) -> Vec<String> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APP_RECORD),
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
    // The model-DSL ledger goes through `emit::diagnostics::push`,
    // which is a no-op outside a scope — so the lowering has to run
    // inside one or the entries are simply lost, and an assertion
    // against them passes for the wrong reason.
    let analyze = roundhouse::session::analyze_and_lower(&mut app);
    // The model-DSL ledger runs in the EMIT, not in the lowering, and
    // `emit::diagnostics::push` is a no-op outside a scope — so the
    // emit has to run inside one or the entries are lost and an
    // assertion against them passes for the wrong reason.
    let (_, emit) = roundhouse::emit::diagnostics::scope(|| ruby::emit_lowered_models(&app));
    analyze
        .iter()
        .chain(emit.iter())
        .map(Diagnostic::to_string)
        .collect()
}

#[test]
fn a_sorbet_annotation_on_a_model_is_not_reported_as_unlowered_dsl() {
    // An abstract base carries `abstract!` and `extend T::Helpers`.
    // The library-class walk has always dropped those; a model never
    // met them until a base outside `app/models` started being
    // classified as one, and then every such base reported its
    // annotations as unlowered model DSL.
    //
    // Asserted on the DIAGNOSTICS, not on the emitted text: the
    // annotations were never emitted either way, so a text assertion
    // passes with this change reverted and proves nothing. The first
    // version of this test made exactly that mistake.
    let diags = diagnostics(&[
        (
            "lib/pkg/pkg_record.rb",
            "class PkgRecord < ApplicationRecord\n  extend T::Helpers\n\n  abstract!\n\n  self.abstract_class = true\nend\n",
        ),
        ("app/models/gauge.rb", "class Gauge < PkgRecord\nend\n"),
    ]);
    let noise: Vec<&String> = diags
        .iter()
        .filter(|d| d.contains("abstract!") || d.contains("T::Helpers") || d.contains("`extend`"))
        .collect();
    assert!(noise.is_empty(), "sorbet annotations reported as model DSL: {noise:?}");
}

#[test]
fn a_tableless_active_model_under_lib_stays_a_library_class() {
    // The narrower rule, and why it is narrower. `classify_class_file`
    // also answers `Model` for a superclass-less class that includes
    // `ActiveModel::Model` — a TABLELESS model. Outside `app/models`
    // that is left as a library class on purpose: a reopen of a
    // framework class from `lib/` includes things the emit handles its
    // own way, and routing it to the model path breaks that. The
    // existing `active_model_model_library_class` cases caught this
    // when the first version of this change was too broad.
    let emitted = emitted(
        &[(
            "lib/rails_ext/embed.rb",
            "class Embed\n  include ActiveModel::Model\n\n  def label\n    \"x\"\n  end\nend\n",
        )],
        "embed.rb",
    );
    assert!(emitted.contains("def label"), "got:\n{emitted}");
    // A model would have been given a table's accessors; this has none.
    assert!(!emitted.contains("def note"), "got:\n{emitted}");
}
