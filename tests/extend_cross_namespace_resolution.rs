//! `extend`/`prepend` of a module from another namespace, inside a
//! library class nested two modules deep with its own app-local
//! superclass, used to be dropped by the Ruby/Spinel emitter — #1114.
//!
//! Since #1094 ("Resolve superclasses and mixins once from Rubydex"),
//! a class's resolved superclass is correctly recognized as an
//! app-local class (not a gem's), so `replays_foreign_class_body`
//! correctly says "no, this class body is modelled" — and that routed
//! every class-body call this emitter doesn't structurally model
//! (`extend`, `prepend`) into `app_extension_calls`'s narrower branch,
//! whose own gate (`lc.parent` must be `None` or a built-in) excluded
//! any class with an ordinary app superclass outright, independently
//! of whether the extended/prepended module resolved at all. Before
//! #1094, the same class's superclass was *mis*resolved as foreign
//! (not yet rewritten to its qualified form), which routed it through
//! the "replay the whole body verbatim" branch instead — accidentally
//! keeping the `extend`/`prepend` line.
//!
//! The fix: `src/ingest/written_names.rs` now resolves `extend` and
//! `prepend` constant arguments (not just `include`) from Rubydex,
//! including inside a library class's own captured `unknown_calls`
//! (previously untouched — only the *structured* `include` list was).
//! `src/emit/ruby/library.rs`'s `app_extension_calls` reads that
//! already-resolved name back, instead of a second, independent
//! written-name guess, and no longer bails out just because the class
//! has an ordinary app-local superclass (that question is
//! `replays_foreign_class_body`'s alone; the caller already routes
//! foreign bodies there before ever calling `app_extension_calls`).
//!
//! #1113 fixed the analogous `include` disagreement for a *model's*
//! relative concern (the `lock!`/`reload` forwarder); this is the
//! `extend`/`prepend` counterpart, for a *library class*'s own emitted
//! body, that #1113 did not cover.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::DiagnosticKind;
use roundhouse::ingest::ingest_app_from_tree;

const BASE: &str = "module Widget\n  module Tools\n    class Base\n    end\n  end\nend\n";

/// `Probe` is nested two modules deep; `Beacon::Pinger` is declared at
/// the top level, under neither `Widget` nor `Widget::Tools` — not a
/// lexical ancestor of the `extend`/`prepend`, found only because
/// Ruby's lookup falls all the way through to the top level.
const PROBE: &str = r#"
module Widget
  module Tools
    class Probe < Base
      # Ruby's prepend puts the module ahead of the class's own method.
      def pinger_configured?
        false
      end
      extend Beacon::Pinger
      prepend Beacon::Pinger
    end
  end
end
"#;

/// The same mixins written rooted (`::Beacon::Pinger`): a rooted path
/// names only the top-level module, never one nested under `Rooted`.
const ROOTED: &str = r#"
module Widget
  module Tools
    class Rooted < Base
      # Ruby's prepend puts the module ahead of the class's own method.
      def pinger_configured?
        false
      end
      extend ::Beacon::Pinger
      prepend ::Beacon::Pinger
    end
  end
end
"#;

const PINGER: &str = "module Beacon\n  module Pinger\n    def pinger_configured?\n      true\n    end\n  end\nend\n";

const GADGET: &str = r#"
class Gadget
  def self.probe_ok?
    Widget::Tools::Probe.pinger_configured?
  end
end
"#;

/// Control: an `extend` of a module nested directly under the
/// extending class's own name, no app-local superclass — the shape
/// `app_extension_calls` was originally written for (#`extend_module_class_methods.rs`'s
/// sibling for a library class). Must keep working.
const SWITCH: &str = r#"
module Widget
  module Tools
    class Switch
      module Flags
        def flag_on?
          true
        end
      end
      extend Flags
    end
  end
end
"#;

const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const ROUTES: &str = "Rails.application.routes.draw do\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema.define do\nend\n";

fn files() -> HashMap<PathBuf, Vec<u8>> {
    [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/models/widget/tools/base.rb", BASE),
        ("app/models/widget/tools/probe.rb", PROBE),
        ("app/models/widget/tools/switch.rb", SWITCH),
        ("app/models/widget/tools/rooted.rb", ROOTED),
        ("lib/beacon/pinger.rb", PINGER),
        ("app/models/gadget.rb", GADGET),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect()
}

fn app() -> roundhouse::App {
    analyzed().0
}

/// The app plus the diagnostics `analyze_and_lower` itself returns.
fn analyzed() -> (roundhouse::App, Vec<roundhouse::diagnostic::Diagnostic>) {
    let mut app = ingest_app_from_tree(files()).expect("ingest tree");
    let lowered = roundhouse::session::analyze_and_lower(&mut app);
    (app, lowered)
}

/// `check`/`--survey`'s diagnostic side is silent either way — #1114's
/// own point is that this check alone proves nothing about the emit.
#[test]
fn analysis_alone_reports_no_error_for_the_cross_namespace_extend() {
    let (app, lowered) = analyzed();
    let lowered: Vec<String> = lowered.into_iter().map(|d| d.to_string()).collect();
    assert!(lowered.is_empty(), "analyze_and_lower: {lowered:?}");
    let diagnostics: Vec<String> = roundhouse::analyze::diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    assert!(diagnostics.is_empty(), "diagnose: {diagnostics:?}");
}

/// The real claim #1114 is about: the Ruby emitter keeps `extend`/
/// `prepend` of the cross-namespace module in `Probe`'s body, with no
/// `lower_residue` warning — and the control (`Switch`'s own-nested
/// `extend`) keeps working the same way.
#[test]
fn the_ruby_emitter_keeps_the_cross_namespace_extend_and_prepend() {
    let app = app();
    let (files, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&app, std::path::Path::new("nonexistent"), roundhouse::project::BuildTarget::Ruby)
    });
    let dropped: Vec<String> = diagnostics
        .iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::LowerResidue { .. }))
        .map(|d| d.message.clone())
        .collect();
    assert!(dropped.is_empty(), "extend/prepend dropped as lower_residue: {dropped:#?}");
    let files = files.expect("target files");
    let (_, probe_src) = files
        .iter()
        .find(|(path, _)| path.ends_with("probe.rb"))
        .unwrap_or_else(|| panic!("no emitted probe.rb in {:?}", files.iter().map(|(p, _)| p).collect::<Vec<_>>()));
    assert!(probe_src.contains("extend Beacon::Pinger"), "{probe_src}");
    assert!(probe_src.contains("prepend Beacon::Pinger"), "{probe_src}");
    let (_, rooted_src) = files.iter().find(|(path, _)| path.ends_with("rooted.rb")).expect("no emitted rooted.rb");
    assert!(rooted_src.contains("extend ::Beacon::Pinger") || rooted_src.contains("extend Beacon::Pinger"), "{rooted_src}");
    assert!(rooted_src.contains("prepend ::Beacon::Pinger") || rooted_src.contains("prepend Beacon::Pinger"), "{rooted_src}");
    let (_, switch_src) = files.iter().find(|(path, _)| path.ends_with("switch.rb")).expect("no emitted switch.rb");
    // `written_names` now resolves this `extend`'s argument too, so
    // the written, same-file-relative `Flags` is emitted as Rubydex's
    // fully qualified answer rather than kept bare — still the same
    // module, so the control's claim (this already-working shape
    // keeps working) is about the `run_ruby` contract below, not the
    // emitted spelling.
    assert!(switch_src.contains("extend Widget::Tools::Switch::Flags"), "{switch_src}");
}

const SCRIPT: &str = r#"
raise "extend not applied" unless Widget::Tools::Probe.pinger_configured?
raise "prepend not applied" unless Widget::Tools::Probe.new.pinger_configured?
raise "rooted extend not applied" unless Widget::Tools::Rooted.pinger_configured?
raise "rooted prepend not applied" unless Widget::Tools::Rooted.new.pinger_configured?
raise "control extend broke" unless Widget::Tools::Switch.flag_on?
puts "cross-namespace extend/prepend contract passed"
"#;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("config/routes.rb", ROUTES)
        .write("app/controllers/application_controller.rb", APPLICATION_CONTROLLER)
        .write("app/models/widget/tools/base.rb", BASE)
        .write("app/models/widget/tools/probe.rb", PROBE)
        .write("app/models/widget/tools/switch.rb", SWITCH)
        .write("app/models/widget/tools/rooted.rb", ROOTED)
        .write("lib/beacon/pinger.rb", PINGER)
        .write("app/models/gadget.rb", GADGET)
}

/// The emitted `ruby` target actually runs: `Probe.pinger_configured?`
/// (via `extend`) and `Probe.new.pinger_configured?` (via `prepend`,
/// which puts `Pinger`'s instance methods ahead of `Probe`'s own in
/// the instance ancestor chain — here there is no override, so the
/// method answers either way, and the only thing under test is that
/// the module is mixed in at all).
#[test]
fn the_emitted_ruby_program_runs_the_cross_namespace_extend_and_prepend() {
    overlay().run_ruby(SCRIPT).assert_passes();
}

/// Same contract, compiled by the native Spinel compiler and run as a
/// native binary. `run_spinel` requires the Spinel toolchain.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn the_spinel_binary_runs_the_cross_namespace_extend_and_prepend() {
    overlay().run_spinel(SCRIPT).assert_passes();
}
