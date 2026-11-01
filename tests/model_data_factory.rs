use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::project::{BuildTarget, target_files};
use roundhouse::ty::Ty;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/model_data_factory.rs"]
mod contract;

fn app(model: &str) -> roundhouse::App {
    let tree = [
        ("app/models/application_record.rb", contract::BASE),
        ("app/models/assignment.rb", model),
        ("db/schema.rb", contract::SCHEMA),
    ].into_iter().map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec())).collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest")
}

#[test]
fn model_body_factory_registers_the_nested_constructor_and_readers() {
    let mut app = app(contract::MODEL);
    let analyzer = Analyzer::new(&app);
    let id = ClassId(Symbol::from("Assignment::Transition"));
    let class = &analyzer.class_registry()[&id];
    assert_eq!(class.class_methods.get(&Symbol::from("new")), Some(&Ty::Class { id, args: vec![].into() }));
    for member in ["assignment", "changed", "notice"] {
        assert_eq!(class.instance_methods.get(&Symbol::from(member)), Some(&Ty::untyped()));
        assert!(!class.instance_methods.contains_key(&Symbol::from(format!("{member}="))));
    }
    roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<_> = roundhouse::analyze::diagnose(&app).into_iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn model_body_factory_executes_after_ruby_emission() {
    contract::overlay().run_ruby(contract::ASSERTIONS).assert_passes();
}

#[test]
fn model_body_factory_sidecars_declare_the_same_nested_class() {
    let mut app = app(contract::MODEL);
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (files, diagnostics) = roundhouse::emit::diagnostics::scope(|| target_files(&app, roundhouse::fixtures::real_blog(), target));
        assert!(!diagnostics.iter().any(|d| d.severity == Severity::Error), "{diagnostics:?}");
        let files = files.expect("supported target");
        let path = if target == BuildTarget::Ruby { "sig/app/models/assignment.rbs" } else { "app/models/assignment.rbs" };
        let sidecar = &files.iter().find(|(name, _)| name == path).expect("model sidecar").1;
        let signatures = roundhouse::rbs::parse_app_signatures(sidecar).expect("RBS parses");
        let methods = &signatures[&ClassId(Symbol::from("Assignment::Transition"))];
        for member in ["new", "assignment", "changed", "notice"] {
            assert!(methods.contains_key(&Symbol::from(member)), "{sidecar}");
        }
    }
}

#[test]
fn model_body_factory_keeps_the_existing_target_refusal() {
    let mut app = app(contract::MODEL);
    roundhouse::session::analyze_and_lower(&mut app);
    for &target in BuildTarget::TRANSPILE {
        if matches!(target, BuildTarget::Ruby | BuildTarget::Spinel | BuildTarget::Futamura) { continue; }
        let (result, diagnostics) = roundhouse::emit::diagnostics::scope(|| target_files(&app, std::path::Path::new("not-a-fixture"), target));
        assert!(result.is_err(), "{target:?} emitted a Data factory");
        assert!(diagnostics.iter().any(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Data.define")), "{target:?}: {diagnostics:?}");
    }
}

#[test]
fn model_body_factories_do_not_admit_dynamic_members_or_shadowed_data() {
    for model in [
        "class Assignment < ApplicationRecord; MEMBER = :notice; Transition = Data.define(MEMBER); end",
        "class Assignment < ApplicationRecord; Transition = Data.define(:notice, :notice); end",
        "class Assignment < ApplicationRecord; Transition = Data.define(:notice=); end",
        "class Assignment < ApplicationRecord; Data = String; Transition = Data.define(:notice); end",
    ] {
        let app = app(model);
        assert!(!Analyzer::new(&app).class_registry().contains_key(&ClassId(Symbol::from("Assignment::Transition"))), "{model}");
    }
}

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn model_body_factory_executes_natively_with_its_sidecar() {
    contract::overlay().run_spinel_with_rbs(contract::ASSERTIONS).assert_passes();
}
