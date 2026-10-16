//! A positional rest (`def f(*args)`, and the anonymous `def f(*)`)
//! is a ruby-family construct. Every other emitter rendered it as ONE
//! required parameter — `named(args: any)`, `func named(_ args: Any?)`
//! — so `f()` and `f(a, b)` no longer matched the declaration and
//! `f(a)` handed the body `a` where Ruby hands it `[a]`, while `check`
//! reported nothing. Those targets now say so; the ruby family, which
//! renders `*args`, does not.
//!
//! A controller method used to have no rest slot at all, so
//! `def pick(*keys)` was emitted as `def pick` on EVERY target. `Action`
//! now carries it (`Action::formal_params`), and the same targets report
//! it there. What still has no slot, required positionals after the
//! rest, is refused at ingest, and ledgered on its own def when a
//! concern splices the method into a controller.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n  end\nend\n";

fn tree(extra: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    let mut files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ];
    files.extend_from_slice(extra);
    files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const MODEL: &str = "class Gauge < ApplicationRecord
  def named(*args)
    \"named\"
  end

  def anon(*)
    \"anon\"
  end

  def plain(a, b = 1)
    \"plain\"
  end
end
";

const HELPER: &str = "module GaugesHelper
  def mark(*)
    \"[mark]\"
  end
end
";

const CONCERN: &str = "module Picker
  extend ActiveSupport::Concern

  private
    def spliced(*keys)
      keys.length
    end
end
";

const CONTROLLER: &str = "class GaugesController < ApplicationController
  include Picker

  def index
    @out = [pick(:a, :b), spliced(:c), plain_pick(1)]
  end

  private

  def pick(*keys)
    keys.length
  end

  def plain_pick(a, b = 1)
    a
  end
end
";

fn ledgered(target: BuildTarget) -> Vec<String> {
    let mut app = ingest_app_from_tree(tree(&[
        ("app/models/gauge.rb", MODEL),
        ("app/helpers/gauges_helper.rb", HELPER),
        ("app/controllers/concerns/picker.rb", CONCERN),
        ("app/controllers/gauges_controller.rb", CONTROLLER),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
        let _ = roundhouse::project::target_files(&app, std::path::Path::new("."), target);
    });
    diags
        .iter()
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .filter(|d| d.contains("rest parameter"))
        .collect()
}

#[test]
fn a_target_without_a_rest_carrier_reports_each_rest_parameter() {
    for target in [
        BuildTarget::Typescript,
        BuildTarget::Swift,
        BuildTarget::Go,
        BuildTarget::Python,
        BuildTarget::Rust,
        BuildTarget::Kotlin,
        BuildTarget::CSharp,
        BuildTarget::Crystal,
        BuildTarget::Elixir,
    ] {
        let entries = ledgered(target);
        // `spliced` once: the controller's copy shares the concern def's
        // name span, so the two report as one entry.
        assert_eq!(entries.len(), 5, "{target:?}: one per rest parameter; got {entries:?}");
        assert!(entries.iter().any(|d| d.contains("`*args` on `named`")), "{target:?}: {entries:?}");
        assert!(entries.iter().any(|d| d.contains("`*` on `anon`")), "{target:?}: {entries:?}");
        assert!(entries.iter().any(|d| d.contains("`*` on `mark`")), "{target:?}: {entries:?}");
        assert!(entries.iter().any(|d| d.contains("`*keys` on `pick`")), "{target:?}: {entries:?}");
        assert!(entries.iter().any(|d| d.contains("`*keys` on `spliced`")), "{target:?}: {entries:?}");
        assert!(!entries.iter().any(|d| d.contains("`plain")), "{target:?}: {entries:?}");
    }
}

#[test]
fn the_ruby_family_does_not_report_what_it_renders() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let entries = ledgered(target);
        assert!(entries.is_empty(), "{target:?} renders `*args`; got {entries:?}");
    }
}

/// The rest itself is carried; what has no slot is still refused, not
/// dropped: positionals after it, and an anonymous `*` the body forwards
/// (ingest would read the bare splat as `*nil`).
#[test]
fn a_controller_formal_without_a_slot_is_refused_rather_than_dropped() {
    for (def, message) in [
        ("def pick(first, *rest, last)", "required parameters after a positional rest on a controller method"),
        ("def pick(*)\n    Array(*)", "anonymous positional rest is not retained"),
        ("def pick((a, b))", "destructured positional parameters are not retained"),
    ] {
        let controller = format!(
            "class GaugesController < ApplicationController\n  def index\n    @out = pick(:a, :b, :c)\n  end\n\n  private\n\n  {def}\n    \"picked\"\n  end\nend\n"
        );
        let err = ingest_app_from_tree(tree(&[("app/controllers/gauges_controller.rb", &controller)]))
            .err()
            .unwrap_or_else(|| panic!("`{def}` must not ingest without its formals"));
        assert!(err.to_string().contains(message), "`{def}`: {err}");
    }
}

fn gauges(app: &roundhouse::App) -> &roundhouse::dialect::Controller {
    app.controllers.iter().find(|c| c.name.0.as_str() == "GaugesController").expect("GaugesController")
}

#[test]
fn a_controller_rest_parameter_is_carried() {
    for (def, rest) in [("def pick(*keys)", "keys"), ("def pick(*)", "__anon_rest_")] {
        let controller = format!(
            "class GaugesController < ApplicationController\n  def index\n    @out = pick(:a, :b, :c)\n  end\n\n  private\n\n  {def}\n    \"picked\"\n  end\nend\n"
        );
        let app = ingest_app_from_tree(tree(&[("app/controllers/gauges_controller.rb", &controller)]))
            .unwrap_or_else(|e| panic!("`{def}`: {e}"));
        let pick = gauges(&app).actions().find(|a| a.name.as_str() == "pick").expect("pick");
        let carried = pick.rest_param.as_ref().map(|r| r.as_str().to_string()).unwrap_or_default();
        assert!(carried.starts_with(rest), "`{def}`: rest {:?}", pick.rest_param);
    }
}

#[test]
fn a_concern_rest_parameter_spliced_into_a_controller_is_carried() {
    let mut app = ingest_app_from_tree(tree(&[
        ("app/controllers/concerns/picker.rb", CONCERN),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  include Picker\n\n  def index\n    @out = spliced(:a, :b)\n  end\nend\n",
        ),
    ]))
    .expect("ingest");
    let spliced = gauges(&app).actions().find(|a| a.name.as_str() == "spliced").expect("spliced");
    assert_eq!(spliced.rest_param.as_ref().map(|r| r.as_str()), Some("keys"));
    assert!(spliced.params.fields.is_empty(), "the rest must not become a required positional");
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags = roundhouse::analyze::diagnose(&app);
    assert!(
        !diags.iter().any(|d| d.to_string().contains("spliced into a controller")),
        "got {diags:?}"
    );
}

#[test]
fn a_concern_positional_after_a_rest_spliced_into_a_controller_is_ledgered() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "app/controllers/concerns/picker.rb",
            "module Picker\n  extend ActiveSupport::Concern\n\n  private\n    def pick(*keys, last)\n      \"picked\"\n    end\nend\n",
        ),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  include Picker\n\n  def index\n    @out = pick(:a, :b)\n  end\nend\n",
        ),
    ]))
    .expect("ingest");
    // No copy whose `last` would bind ahead of `*keys`.
    assert!(gauges(&app).actions().all(|a| a.name.as_str() != "pick"), "the misbound copy must not be spliced");
    // The `check` path: analyze without the post-analyze lowerings.
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags = roundhouse::analyze::diagnose(&app);
    let found: Vec<String> = diags
        .iter()
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .filter(|d| d.contains("spliced into a controller"))
        .collect();
    assert_eq!(found.len(), 1, "got {found:?}");
}
