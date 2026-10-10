//! Library-class Data factories retain their declaration identity and lexical owner.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::dialect::LibraryClassOrigin;
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ty::Ty;

#[path = "support/data_factory.rs"]
mod data_factory;

fn ingest(source: &str, action: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("app/services/factory_examples.rb"),
            source.as_bytes().to_vec(),
        ),
        (
            PathBuf::from("app/controllers/data_probe_controller.rb"),
            format!("class DataProbeController < ApplicationController\n  def index\n    @result = {action}\n  end\nend\n").into_bytes(),
        ),
    ]
    .into_iter()
    .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest")
}

#[test]
fn literal_data_declarations_register_exact_members_without_writers() {
    let app = ingest(
        data_factory::DECLARATIONS,
        "FactoryExamples::First::Result.new(\"first\", 1.0, false)",
    );
    let analyzer = Analyzer::new(&app);
    let classes = analyzer.class_registry();
    for (name, members) in [
        (
            "FactoryExamples::First::Result",
            vec!["name", "score", "enabled"],
        ),
        ("FactoryExamples::Second::Result", vec!["name"]),
        ("FactoryExamples::Empty::Result", vec![]),
    ] {
        let id = ClassId(Symbol::from(name));
        let class = &classes[&id];
        assert_eq!(
            class.class_methods.get(&Symbol::from("new")),
            Some(&Ty::Class { id, args: vec![] })
        );
        for member in members {
            assert_eq!(
                class.instance_methods.get(&Symbol::from(member)),
                Some(&Ty::Untyped)
            );
            assert!(
                !class
                    .instance_methods
                    .contains_key(&Symbol::from(format!("{member}=")))
            );
        }
    }
    for alias in [
        "Result",
        "FactoryExamples::First::Alias",
        "FactoryExamples::First::ChainedAlias",
    ] {
        assert!(
            !classes.contains_key(&ClassId(Symbol::from(alias))),
            "{alias}"
        );
    }
}

#[test]
fn block_data_classes_retain_ordered_members_as_nominal_origin() {
    let source = r#"class FactoryExamples
  ContentKey = Data.define(:digest, :source) do
    def cache_key = digest
  end
end
"#;
    let app = ingest(
        source,
        "FactoryExamples::ContentKey.new(\"hash\", \"body\")",
    );
    let class = app
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "FactoryExamples::ContentKey")
        .expect("the Data block becomes its own library class");

    assert_eq!(
        class.origin,
        Some(LibraryClassOrigin::DataFactory {
            members: vec![Symbol::from("digest"), Symbol::from("source")],
        })
    );
    assert!(
        class
            .methods
            .iter()
            .any(|method| method.name.as_str() == "cache_key"),
        "source-defined Data block methods remain on the nominal class"
    );
}

#[test]
fn factories_outside_the_literal_subset_are_not_admitted() {
    let other = r#"module Other
  class Data
    def self.define(name)
      name
    end
  end
  class Shadowed
    Result = Data.define(:name)
    Builtin = ::Data.define(:name)

    def self.build
      Builtin.new(name: "builtin")
    end
  end
end
"#;
    let dynamic = r#"class DynamicFactory
  MEMBER = :name
  Result = Data.define(MEMBER)
  Duplicate = Data.define(:name, :name)
  Writer = Data.define(:name=)
  EmptyName = Data.define(:"")
  BacktickName = Data.define(:"a`b")
  OperatorName = Data.define(:":")

  def self.build
    Result.new(name: "dynamic")
  end
end
"#;
    let with_block = r#"class DynamicBlockFactory
  WithBlock = Data.define(:name) { "custom" }

  def self.block_result
    WithBlock.new(name: "blocked")
  end
end
"#;
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "app/services/factory_examples.rb",
            data_factory::DECLARATIONS,
        ),
        ("app/services/other.rb", other),
        ("app/services/dynamic_factory.rb", dynamic),
        ("app/services/dynamic_block_factory.rb", with_block),
    ]
    .into_iter()
    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest literal subset");
    for (owner, constant) in [
        ("FactoryExamples::First", "Result"),
        ("Other::Shadowed", "Builtin"),
        ("DynamicFactory", "Result"),
        ("DynamicFactory", "Duplicate"),
        ("DynamicFactory", "Writer"),
        ("DynamicFactory", "EmptyName"),
        ("DynamicFactory", "BacktickName"),
        ("DynamicFactory", "OperatorName"),
        ("DynamicBlockFactory", "WithBlock"),
    ] {
        assert!(
            app.library_classes
                .iter()
                .any(|class| class.name.0.as_str() == owner
                    && class
                        .constants
                        .iter()
                        .any(|(name, _)| name.as_str() == constant)),
            "{owner}::{constant} was not ingested"
        );
    }
    let analyzer = Analyzer::new(&app);
    for name in [
        "Other::Shadowed::Result",
        "DynamicFactory::Result",
        "DynamicBlockFactory::WithBlock",
        "DynamicFactory::Duplicate",
        "DynamicFactory::Writer",
        "DynamicFactory::EmptyName",
        "DynamicFactory::BacktickName",
        "DynamicFactory::OperatorName",
    ] {
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from(name))),
            "{name}"
        );
    }
    assert!(
        analyzer
            .class_registry()
            .contains_key(&ClassId(Symbol::from("Other::Shadowed::Builtin")))
    );
}

#[test]
fn reopening_core_data_does_not_admit_factories() {
    for (path, source) in [
        (
            "app/lib/data.rb",
            "class Data\n  def self.define(name); name; end\nend\n",
        ),
        (
            "app/models/data.rb",
            "class Data < ApplicationRecord\n  def self.define(name); name; end\nend\n",
        ),
        (
            "app/controllers/data_controller.rb",
            "class Data < ApplicationController\n  def define(name); name; end\nend\n",
        ),
        (
            "test/models/data_test.rb",
            "class Data < ActiveSupport::TestCase\n  def define(name); name; end\nend\n",
        ),
        (
            "test/models/data_test.rb",
            "class DataTest < ActiveSupport::TestCase; end\nclass Data; end\n",
        ),
    ] {
        let tree: HashMap<PathBuf, Vec<u8>> = [
            (
                PathBuf::from("app/services/factory_examples.rb"),
                data_factory::DECLARATIONS.as_bytes().to_vec(),
            ),
            (PathBuf::from(path), source.as_bytes().to_vec()),
        ]
        .into_iter()
        .collect();
        let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
        let analyzer = Analyzer::new(&app);
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from("FactoryExamples::First::Result"))),
            "{path}: {source}"
        );
    }
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("app/services/factory_examples.rb"),
            data_factory::DECLARATIONS.as_bytes().to_vec(),
        ),
        (
            PathBuf::from("test/models/factory_shadow_test.rb"),
            b"class FactoryShadowTest < ActiveSupport::TestCase\n  class Data; end\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest lexical namesake");
    assert!(
        Analyzer::new(&app)
            .class_registry()
            .contains_key(&ClassId(Symbol::from("FactoryExamples::First::Result")))
    );
}

#[test]
fn reopening_an_alias_of_core_data_does_not_admit_factories() {
    for source in [
        "module CoreMutation; Kind = ::Data; class Kind; def self.define(member); member; end; end; end",
        "module CoreMutation; Kind = ::Data; Chained = Kind; end\nclass CoreMutation::Chained; def self.define(member); member; end; end",
    ] {
        let tree: HashMap<PathBuf, Vec<u8>> = [
            (
                PathBuf::from("app/services/factory_examples.rb"),
                data_factory::DECLARATIONS.as_bytes().to_vec(),
            ),
            (
                PathBuf::from("app/lib/core_mutation.rb"),
                source.as_bytes().to_vec(),
            ),
            (
                PathBuf::from("app/controllers/data_probe_controller.rb"),
                b"class DataProbeController < ApplicationController\n  def index\n    @result = FactoryExamples::First::Result.new(name: \"invalid\")\n  end\nend\n".to_vec(),
            ),
        ].into_iter().collect();
        let mut app =
            roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest alias reopening");
        assert!(
            !Analyzer::new(&app)
                .class_registry()
                .contains_key(&ClassId(Symbol::from("FactoryExamples::First::Result"))),
            "{source}"
        );
        roundhouse::session::analyze_and_lower(&mut app);
        let diagnostics = roundhouse::analyze::diagnose(&app);
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.severity
                == roundhouse::diagnostic::Severity::Error
                && diagnostic
                    .message
                    .contains("FactoryExamples::First::Result")),
            "{source}: {diagnostics:?}"
        );
    }
}

#[test]
fn named_singleton_overrides_of_core_data_do_not_admit_factories() {
    for source in [
        "def Data.define(member); member; end\nclass CoreMutation; end",
        "class CoreMutation; def Data.define(member); member; end; end",
        "module CoreMutation; Kind = ::Data; def Kind.define(member); member; end; end",
        "module CoreMutation; Kind = ::Data; Chained = Kind; end\ndef (CoreMutation::Chained).define(member); member; end",
    ] {
        assert!(
            ruby_prism::parse(source.as_bytes())
                .errors()
                .next()
                .is_none(),
            "{source}"
        );
        let tree: HashMap<PathBuf, Vec<u8>> = [
            (
                PathBuf::from("app/services/factory_examples.rb"),
                data_factory::DECLARATIONS.as_bytes().to_vec(),
            ),
            (
                PathBuf::from("app/lib/core_mutation.rb"),
                source.as_bytes().to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        let app =
            roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest singleton override");
        assert!(
            app.library_classes
                .iter()
                .any(|class| class.name.0.as_str() == "FactoryExamples::First"),
            "{source}"
        );
        assert!(
            !Analyzer::new(&app)
                .class_registry()
                .contains_key(&ClassId(Symbol::from("FactoryExamples::First::Result"))),
            "{source}"
        );
    }
    for source in [
        "module Other; class Data; end; def Data.define(member); member; end; end",
        "module Other; class Data; end; Kind = Data; end\ndef (Other::Kind).define(member); member; end",
    ] {
        assert!(
            ruby_prism::parse(source.as_bytes())
                .errors()
                .next()
                .is_none(),
            "{source}"
        );
        let tree: HashMap<PathBuf, Vec<u8>> = [
            (
                PathBuf::from("app/services/factory_examples.rb"),
                data_factory::DECLARATIONS.as_bytes().to_vec(),
            ),
            (
                PathBuf::from("app/lib/other.rb"),
                source.as_bytes().to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        let app = roundhouse::ingest::ingest_app_from_tree(tree)
            .expect("ingest lexical singleton namesake");
        assert!(
            Analyzer::new(&app)
                .class_registry()
                .contains_key(&ClassId(Symbol::from("FactoryExamples::First::Result"))),
            "{source}"
        );
    }
}

#[test]
fn singleton_class_reopenings_of_core_data_do_not_admit_factories() {
    for source in [
        "class << Data; def define(member); member; end; end\nclass CoreMutation; end",
        "class CoreMutation; class << Data; def define(member); member; end; end; end",
        "module CoreMutation; Kind = ::Data; class << Kind; def define(member); member; end; end; end",
        "module CoreMutation; Kind = ::Data; Chained = Kind; end\nclass CoreMutation; class << Chained; def define(member); member; end; end; end",
    ] {
        assert!(
            ruby_prism::parse(source.as_bytes())
                .errors()
                .next()
                .is_none(),
            "{source}"
        );
        let tree: HashMap<PathBuf, Vec<u8>> = [
            (
                PathBuf::from("app/services/factory_examples.rb"),
                data_factory::DECLARATIONS.as_bytes().to_vec(),
            ),
            (
                PathBuf::from("app/lib/core_mutation.rb"),
                source.as_bytes().to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        let app =
            roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest singleton reopening");
        assert!(
            app.library_classes
                .iter()
                .any(|class| class.name.0.as_str() == "FactoryExamples::First"),
            "{source}"
        );
        assert!(
            !Analyzer::new(&app)
                .class_registry()
                .contains_key(&ClassId(Symbol::from("FactoryExamples::First::Result"))),
            "{source}"
        );
    }
    for source in [
        "module Other; class Data; end; class << Data; def define(member); member; end; end; end",
        "module Other; class Data; end; Kind = Data; class << Kind; def define(member); member; end; end; end",
    ] {
        assert!(
            ruby_prism::parse(source.as_bytes())
                .errors()
                .next()
                .is_none(),
            "{source}"
        );
        let tree: HashMap<PathBuf, Vec<u8>> = [
            (
                PathBuf::from("app/services/factory_examples.rb"),
                data_factory::DECLARATIONS.as_bytes().to_vec(),
            ),
            (
                PathBuf::from("app/lib/other.rb"),
                source.as_bytes().to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        let app = roundhouse::ingest::ingest_app_from_tree(tree)
            .expect("ingest lexical singleton namesake");
        assert!(
            Analyzer::new(&app)
                .class_registry()
                .contains_key(&ClassId(Symbol::from("FactoryExamples::First::Result"))),
            "{source}"
        );
    }
}

#[test]
fn a_result_name_in_another_owner_does_not_answer_an_unresolved_constant() {
    let mut app = ingest(
        data_factory::DECLARATIONS,
        "Unrelated::Result.new(name: \"wrong owner\")",
    );
    roundhouse::session::analyze_and_lower(&mut app);
    let diagnostics = roundhouse::analyze::diagnose(&app);
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic.severity
            == roundhouse::diagnostic::Severity::Error
            && diagnostic.message.contains("Unrelated::Result")),
        "{diagnostics:?}"
    );
}

#[test]
fn rehomed_file_level_factories_remain_unsupported() {
    let source = format!(
        "GlobalResult = Data.define(:name)\n{}",
        data_factory::DECLARATIONS
    );
    let mut app = ingest(&source, "GlobalResult.new(name: \"global\")");
    assert!(
        !Analyzer::new(&app)
            .class_registry()
            .contains_key(&ClassId(Symbol::from("GlobalResult")))
    );
    roundhouse::session::analyze_and_lower(&mut app);
    let diagnostics = roundhouse::analyze::diagnose(&app);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == roundhouse::diagnostic::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn factories_outside_library_classes_remain_unsupported() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "app/models/factory_model.rb",
            r#"class FactoryModel < ApplicationRecord
  Result = Data.define(:name)
  FactoryModel::Qualified = Data.define(:name)
end
"#,
        ),
        (
            "app/controllers/factory_owner_controller.rb",
            r#"class FactoryOwnerController < ApplicationController
  Result = Data.define(:name)
  FactoryOwnerController::Qualified = Data.define(:name)

  def index
    @model = FactoryModel::Result.new(name: "model")
    @controller = FactoryOwnerController::Result.new(name: "controller")
    @test = FactoryTest::Result.new(name: "test")
    @companion = FactoryCompanion::Result.new(name: "companion")
  end
end
"#,
        ),
        (
            "test/models/factory_test.rb",
            r#"class FactoryTest < ActiveSupport::TestCase
  Result = Data.define(:name)
end
class FactoryCompanion
  Result = Data.define(:name)
end
"#,
        ),
    ]
    .into_iter()
    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    assert_eq!(app.test_modules.len(), 1);
    assert_eq!(app.test_modules[0].inner_classes.len(), 1);
    let analyzer = Analyzer::new(&app);
    for name in [
        "FactoryModel::Result",
        "FactoryModel::Qualified",
        "FactoryOwnerController::Result",
        "FactoryOwnerController::Qualified",
        "FactoryTest::Result",
        "FactoryCompanion::Result",
    ] {
        assert!(
            !analyzer
                .class_registry()
                .contains_key(&ClassId(Symbol::from(name))),
            "{name}"
        );
    }
    roundhouse::session::analyze_and_lower(&mut app);
    let diagnostics = roundhouse::analyze::diagnose(&app);
    for name in [
        "FactoryModel::Result",
        "FactoryOwnerController::Result",
        "FactoryTest::Result",
        "FactoryCompanion::Result",
    ] {
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.severity
                == roundhouse::diagnostic::Severity::Error
                && diagnostic.message.contains(name)),
            "{name}: {diagnostics:?}"
        );
    }
}

#[test]
fn core_data_new_remains_unsupported() {
    use roundhouse::diagnostic::{DiagnosticKind, Severity};
    let source = format!(
        "{}\nclass CoreAlias; Kind = ::Data; end\n",
        data_factory::DECLARATIONS
    );
    for action in ["Data.new", "::Data.new", "CoreAlias::Kind.new"] {
        let mut app = ingest(&source, action);
        roundhouse::session::analyze_and_lower(&mut app);
        let diagnostics = roundhouse::analyze::diagnose(&app);
        assert!(diagnostics.iter().any(|diagnostic| diagnostic.severity == Severity::Error
            && matches!(&diagnostic.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Data.new")), "{action}: {diagnostics:?}");
    }
    let mut app = ingest(
        data_factory::DECLARATIONS,
        "FactoryExamples::First::Result.new(name: \"valid\", score: 1.0, enabled: true)",
    );
    roundhouse::session::analyze_and_lower(&mut app);
    let diagnostics = roundhouse::analyze::diagnose(&app);
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn admitted_data_factories_are_rejected_before_unverified_target_emission() {
    use roundhouse::diagnostic::{DiagnosticKind, Severity};
    use roundhouse::project::{BuildTarget, target_files};

    let mut app = ingest(
        data_factory::DECLARATIONS,
        "FactoryExamples::First::Result.new(\"first\", 1.0, false)",
    );
    roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    for &target in BuildTarget::TRANSPILE {
        if matches!(target, BuildTarget::Ruby | BuildTarget::Spinel) {
            continue;
        }
        let (result, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, std::path::Path::new("not-a-fixture"), target)
        });
        assert!(
            result.is_err(),
            "{target:?} silently emitted a Data factory"
        );
        assert_eq!(diagnostics.len(), 3, "{target:?}: {diagnostics:?}");
        for diagnostic in diagnostics {
            assert_eq!(diagnostic.severity, Severity::Error);
            assert!(!diagnostic.span.is_synthetic(), "{diagnostic:?}");
            assert!(
                matches!(&diagnostic.kind, DiagnosticKind::Unsupported { construct, target: Some(name), .. }
                if construct.as_str() == "Data.define" && name.as_str() == target.as_str()),
                "{diagnostic:?}"
            );
        }
    }
}

#[test]
fn ruby_and_spinel_emit_declared_factory_types_without_data_errors() {
    use roundhouse::diagnostic::Severity;
    use roundhouse::project::{BuildTarget, target_files};

    let mut app = ingest(
        data_factory::DECLARATIONS,
        "FactoryExamples::First::Result.new(\"first\", 1.0, false)",
    );
    roundhouse::session::analyze_and_lower(&mut app);
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (result, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, roundhouse::fixtures::real_blog(), target)
        });
        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error),
            "{target:?}: {diagnostics:?}"
        );
        let files = result.expect("supported target files");
        let prefix = if target == BuildTarget::Ruby {
            "sig/"
        } else {
            ""
        };
        let path = format!("{prefix}app/models/factory_examples/first.rbs");
        let sidecar = &files
            .iter()
            .find(|(name, _)| name == &path)
            .expect("factory sidecar")
            .1;
        assert!(
            sidecar.contains("class Result < ::Data"),
            "{target:?}: {sidecar}"
        );
        let signatures = roundhouse::rbs::parse_app_signatures(sidecar).expect("target RBS parses");
        assert!(
            signatures.contains_key(&ClassId(Symbol::from("FactoryExamples::First::Result"))),
            "{target:?}: {sidecar}"
        );
    }
}
