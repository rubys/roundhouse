use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::project::{BuildTarget, target_files};
use roundhouse::ty::Ty;

#[path = "support/stale_object_error.rs"]
mod contract;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app(source: &str) -> roundhouse::App {
    let source = source.replace(
        "class StaleProbe\n",
        "class StaleProbe < ApplicationRecord\n",
    );
    let tree = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/models/stale_record.rb", contract::RECORD),
        ("app/models/stale_probe.rb", source.as_str()),
    ]
    .into_iter()
    .map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest")
}

#[test]
fn stale_object_error_has_its_parent_and_payload_readers() {
    let mut app = app(contract::SOURCE);
    let analyzer = Analyzer::new(&app);
    let info = &analyzer.class_registry()[&ClassId(Symbol::from("ActiveRecord::StaleObjectError"))];
    assert_eq!(
        info.parent,
        Some(ClassId(Symbol::from("ActiveRecord::ActiveRecordError")))
    );
    for name in ["record", "attempted_action"] {
        assert_eq!(
            info.instance_methods.get(&Symbol::from(name)),
            Some(&Ty::untyped())
        );
    }
    let mut diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    diagnostics.extend(roundhouse::analyze::diagnose(&app));
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn stale_object_error_executes_after_ruby_emission() {
    contract::overlay()
        .run_ruby(contract::ASSERTIONS)
        .assert_passes();
}

#[test]
fn stale_object_error_does_not_admit_unknown_methods() {
    let mut app = app(&contract::SOURCE.replace("error.message", "error.no_such_stale_method"));
    Analyzer::new(&app).analyze(&mut app);
    let diagnostics = roundhouse::analyze::diagnose(&app);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("no_such_stale_method")),
        "{diagnostics:?}"
    );
}

#[test]
fn stale_object_error_stays_reported_on_strict_targets() {
    let mut app = app(contract::SOURCE);
    roundhouse::session::analyze_and_lower(&mut app);
    for &target in BuildTarget::TRANSPILE {
        if matches!(
            target,
            BuildTarget::Ruby
                | BuildTarget::Jruby
                | BuildTarget::Spinel
                | BuildTarget::Roda
                | BuildTarget::Futamura
        ) {
            continue;
        }
        let (_result, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, std::path::Path::new("not-a-fixture"), target)
        });
        assert!(diagnostics.iter().any(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "ruby_family_runtime_constant")), "{target:?}: {diagnostics:?}");
    }
}

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn stale_object_error_executes_natively_with_its_sidecar() {
    contract::overlay()
        .run_spinel_with_rbs(contract::ASSERTIONS)
        .assert_passes();
}
