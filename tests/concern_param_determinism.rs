//! A module method's `.rbs` signature does not depend on the order its
//! call sites are folded in (#209).
//!
//! `Reporting#note(message, label = "none")` is called from two
//! includers: `Uploader` passes `JSON.parse(...)` (typed `untyped`) and
//! `Importer` passes `args.first` (left unresolved, a `Var`). The join
//! of the two observations depended on which arrived first, and the
//! includers were folded in `HashMap` order, so repeated emits of the
//! same app alternated between `?String label` and `?untyped label`.
//! The unresolved observation carries no information, so the answer is
//! the one `Uploader` alone produces: `?String label`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const WIDGETS_CONTROLLER: &str =
    "class WidgetsController < ApplicationController\n  def show\n    render json: { ok: 1 }\n  end\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";
const ROUTES: &str = "Rails.application.routes.draw do\n  root \"widgets#show\"\nend\n";

const REPORTING: &str = r##"module Reporting
  def note(message, label = "none")
    Rails.logger.info("#{label}: #{message}")
  end
end
"##;

const IMPORTER: &str = r#"class Importer
  include Reporting

  def perform(*args)
    note(args.first)
  end
end
"#;

const UPLOADER: &str = r#"class Uploader
  include Reporting

  def perform(payload)
    note(JSON.parse(payload))
  end
end
"#;

const EXPECTED: &str = "def note: (untyped message, ?String label) -> untyped";

/// `app/models/reporting.rbs` for the base app plus `models`.
fn reporting_rbs(models: &[(&str, &str)]) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, content) in [
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/models/reporting.rb", REPORTING),
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
    ]
    .into_iter()
    .chain(models.iter().copied())
    {
        tree.insert(PathBuf::from(path), content.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel)
        .expect("spinel files")
        .into_iter()
        .find(|(p, _)| p == "app/models/reporting.rbs")
        .map(|(_, c)| c)
        .expect("reporting.rbs emitted")
}

fn note_sig(rbs: &str) -> &str {
    rbs.lines()
        .map(str::trim)
        .find(|l| l.starts_with("def note: "))
        .unwrap_or_else(|| panic!("no sig for note:\n{rbs}"))
}

#[test]
fn two_includers_emit_the_same_signature_every_time() {
    // Each analysis builds fresh `HashMap`s with fresh random seeds, so
    // repeated emits walk the includers in varying orders.
    let models = [("app/models/importer.rb", IMPORTER), ("app/models/uploader.rb", UPLOADER)];
    let first = reporting_rbs(&models);
    assert_eq!(note_sig(&first), EXPECTED, "{first}");
    for _ in 0..12 {
        assert_eq!(reporting_rbs(&models), first);
    }
}

#[test]
fn an_unresolved_argument_does_not_erase_an_untyped_one() {
    // The same two observations from one class, in both source orders.
    // Call sites in one class are folded in source order, so this pins
    // the join itself rather than relying on hash order.
    let untyped_first = "class Worker\n  include Reporting\n\n  def perform(payload, *args)\n    note(JSON.parse(payload))\n    note(args.first)\n  end\nend\n";
    let unresolved_first = "class Worker\n  include Reporting\n\n  def perform(payload, *args)\n    note(args.first)\n    note(JSON.parse(payload))\n  end\nend\n";
    let a = reporting_rbs(&[("app/models/worker.rb", untyped_first)]);
    let b = reporting_rbs(&[("app/models/worker.rb", unresolved_first)]);
    assert_eq!(note_sig(&a), EXPECTED, "{a}");
    assert_eq!(a, b);
}
