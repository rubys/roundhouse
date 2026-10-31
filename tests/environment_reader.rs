#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const SOURCE: &str = r#"class EnvironmentReader
  def initialize(environment: ENV)
    @environment = environment
  end

  def read(name)
    environment.fetch(name, "fallback")
  end

  def self.injected_read(name)
    new(environment: {"RH_ENV_READER_1236" => "injected"}).read(name)
  end

  def self.direct_read(name)
    ENV.fetch(name, "fallback")
  end

  private
  attr_reader :environment
end
"#;

const SCRIPT: &str = r#"require_relative "app/models/environment_reader"
ENV["RH_ENV_READER_1236"] = "present"
ENV.delete("RH_ENV_READER_MISSING_1236")
reader = EnvironmentReader.new
raise "present" unless reader.read("RH_ENV_READER_1236") == "present"
raise "fallback" unless reader.read("RH_ENV_READER_MISSING_1236") == "fallback"
raise "injection" unless EnvironmentReader.injected_read("RH_ENV_READER_1236") == "injected"
raise "injected fallback" unless EnvironmentReader.injected_read("RH_ENV_READER_MISSING_1236") == "fallback"
raise "direct" unless EnvironmentReader.direct_read("RH_ENV_READER_1236") == "present"
raise "direct fallback" unless EnvironmentReader.direct_read("RH_ENV_READER_MISSING_1236") == "fallback"
raise "global mutation" unless ENV["RH_ENV_READER_1236"] == "present"
puts "PASS"
"#;

fn app(source: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/models/environment_reader.rb", source)
}

fn reader_contract(source: &str) -> String {
    let (emitted, errors) = app(source).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    fn append_rbs(path: &std::path::Path, out: &mut String) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                append_rbs(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rbs") {
                out.push_str(&std::fs::read_to_string(path).unwrap());
            }
        }
    }
    let mut rbs = String::new();
    append_rbs(&emitted.join("app/models"), &mut rbs);
    rbs
}

#[test]
fn environment_defaults_and_injected_hashes_keep_their_values() {
    app(SOURCE).run_ruby(SCRIPT).assert_passes();
}

#[test]
fn an_environment_reader_uses_the_hash_value_contract() {
    let rbs = reader_contract(SOURCE);
    assert!(
        rbs.contains("attr_reader environment: Hash[String, String]"),
        "{rbs}"
    );
    assert!(
        !rbs.contains("attr_reader environment: singleton(ENV)"),
        "{rbs}"
    );
}

#[test]
fn a_source_value_named_env_keeps_its_string_contract() {
    let source = format!(
        "ENV = \"local\"\n{}",
        SOURCE
            .replace(
                "environment.fetch(name, \"fallback\")",
                "environment + name"
            )
            .replace(
                "    new(environment: {\"RH_ENV_READER_1236\" => \"injected\"}).read(name)",
                "    new.read(name)"
            )
            .replace("ENV.fetch(name, \"fallback\")", "ENV + name")
    );
    assert!(reader_contract(&source).contains("attr_reader environment: String"));
}

#[test]
fn a_lexical_env_value_keeps_its_string_contract() {
    let source = format!(
        "module EnvScope\nENV = \"local\"\n{}\nend\n",
        SOURCE
            .replace(
                "environment.fetch(name, \"fallback\")",
                "environment + name"
            )
            .replace(
                "    new(environment: {\"RH_ENV_READER_1236\" => \"injected\"}).read(name)",
                "    new.read(name)"
            )
            .replace("ENV.fetch(name, \"fallback\")", "ENV + name")
    );
    assert!(reader_contract(&source).contains("attr_reader environment: String"));
}

#[test]
fn a_rooted_env_value_bypasses_the_lexical_shadow() {
    let source = format!(
        "module EnvScope\nENV = \"local\"\n{}\nend\n",
        SOURCE
            .replace("environment: ENV", "environment: ::ENV")
            .replace(
                "ENV.fetch(name, \"fallback\")",
                "::ENV.fetch(name, \"fallback\")"
            )
    );
    assert!(reader_contract(&source).contains("attr_reader environment: Hash[String, String]"));
}

#[test]
fn a_source_class_named_env_remains_a_class_object() {
    let source = format!(
        "class ENV\n  def self.fetch(name, fallback)\n    \"custom\"\n  end\nend\n{}",
        SOURCE.replace(
            "    new(environment: {\"RH_ENV_READER_1236\" => \"injected\"}).read(name)",
            "    new.read(name)"
        )
    );
    assert!(reader_contract(&source).contains("attr_reader environment: singleton(ENV)"));
}

#[test]
fn direct_env_calls_do_not_gain_hash_only_methods() {
    let source = "class EnvironmentReader\n  def self.invalid\n    ENV.merge({\"key\" => \"value\"})\n  end\nend\n";
    let (_, analyzed, _) = app(source).emit_with_app(roundhouse::project::BuildTarget::Spinel);
    let class = analyzed
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "EnvironmentReader")
        .unwrap();
    let method = class
        .methods
        .iter()
        .find(|method| method.name.as_str() == "invalid")
        .unwrap();
    if let Some(roundhouse::ty::Ty::Fn { ret, .. }) = &method.signature {
        assert!(
            matches!(
                ret.as_ref(),
                roundhouse::ty::Ty::Untyped | roundhouse::ty::Ty::Var { .. }
            ),
            "{ret:?}"
        );
    } else {
        assert!(method.signature.is_none());
    }
}

#[test]
fn an_env_value_does_not_gain_hash_only_methods() {
    let source = "class EnvironmentReader\n  def initialize(environment: ENV)\n    @environment = environment\n  end\n  def invalid\n    @environment.merge({\"extra\" => \"added\"})\n  end\nend\n";
    let (_, analyzed, _) = app(source).emit_with_app(roundhouse::project::BuildTarget::Spinel);
    let class = analyzed
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "EnvironmentReader")
        .unwrap();
    let method = class
        .methods
        .iter()
        .find(|method| method.name.as_str() == "invalid")
        .unwrap();
    if let Some(roundhouse::ty::Ty::Fn { ret, .. }) = &method.signature {
        assert!(
            matches!(
                ret.as_ref(),
                roundhouse::ty::Ty::Untyped | roundhouse::ty::Ty::Var { .. }
            ),
            "{ret:?}"
        );
    } else {
        assert!(method.signature.is_none());
    }
}

#[test]
fn a_source_hash_named_env_keeps_its_hash_methods() {
    let source = "ENV = {\"key\" => \"value\"}\nclass EnvironmentReader\n  def self.augment\n    ENV.merge({\"extra\" => \"added\"})\n  end\nend\n";
    let (_, analyzed, _) = app(source).emit_with_app(roundhouse::project::BuildTarget::Spinel);
    let class = analyzed
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "EnvironmentReader")
        .unwrap();
    let method = class
        .methods
        .iter()
        .find(|method| method.name.as_str() == "augment")
        .unwrap();
    let Some(roundhouse::ty::Ty::Fn { ret, .. }) = &method.signature else {
        panic!("no signature")
    };
    assert!(
        matches!(ret.as_ref(), roundhouse::ty::Ty::Hash { .. }),
        "{ret:?}"
    );
}

#[test]
fn a_source_class_named_env_keeps_its_own_methods() {
    let source = "class ENV\n  def self.merge(value)\n    \"custom\"\n  end\nend\nclass EnvironmentReader\n  def self.augment\n    ENV.merge({\"extra\" => \"added\"})\n  end\nend\n";
    let (_, analyzed, _) = app(source).emit_with_app(roundhouse::project::BuildTarget::Spinel);
    let class = analyzed
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "EnvironmentReader")
        .unwrap();
    let method = class
        .methods
        .iter()
        .find(|method| method.name.as_str() == "augment")
        .unwrap();
    let Some(roundhouse::ty::Ty::Fn { ret, .. }) = &method.signature else {
        panic!("no signature")
    };
    assert_eq!(ret.as_ref(), &roundhouse::ty::Ty::Str);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn environment_readers_run_with_emitted_rbs() {
    app(SOURCE).run_spinel_with_rbs(SCRIPT).assert_passes();
}
