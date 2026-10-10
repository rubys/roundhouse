//! The registry accepts SecurityUtils; its emitted runtime must exist too.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/comparisons\", to: \"comparisons#index\"\nend\n")
        .write("app/services/signatures.rb", r#"class Signatures
  def self.match(left, right)
    ActiveSupport::SecurityUtils.secure_compare(left, right)
  end

  def self.fixed_length_match(left, right)
    ActiveSupport::SecurityUtils.fixed_length_secure_compare(left, right)
  end
end
"#)
        .write("app/controllers/comparisons_controller.rb", r#"class ComparisonsController < ApplicationController
  def index
    render plain: Signatures.match("same", "same").to_s
  end
end
"#)
}

const STRING_COMPARISONS: &str = r#"
require_relative "app/controllers/comparisons_controller"
controller = ComparisonsController.new
controller.process_action(:index)
raise "controller comparison failed" unless controller.body == "true"
cases = [
  ["abc", "abc", true], ["abc", "xbc", false], ["abc", "abd", false],
  ["abc", "ab", false], ["ab", "abc", false], ["", "", true],
  ["", "a", false], ["é", "é", true], ["é", "ê", false],
  ["é", "ab", false], ["a\0b", "a\0b", true], ["a\0b", "a\0c", false],
  ["é", "\xc3\xa9".b, true]
]
cases.each do |left, right, expected|
  actual = Signatures.match(left, right)
  raise "comparison changed: #{[left, right, actual].inspect}" unless actual == expected
end
puts "SecurityUtils comparison passed (13 vectors and controller)"
"#;

const FIXED_LENGTH_COMPARISONS: &str = r#"
cases = [
  ["abc", "abc", true], ["abc", "abd", false], ["", "", true],
  ["é", "\xc3\xa9".b, true], ["é", "ê", false],
  ["a\0b", "a\0b", true], ["a\0b", "a\0c", false]
]
cases.each do |left, right, expected|
  actual = Signatures.fixed_length_match(left, right)
  raise "fixed-length comparison changed: #{[left, right, actual].inspect}" unless actual == expected
end
begin
  Signatures.fixed_length_match("é", "x")
rescue ArgumentError => error
  raise "wrong mismatch error: #{error.message.inspect}" unless error.message == "string length mismatch."
else
  raise "different byte lengths were accepted"
end
puts "SecurityUtils fixed-length comparison passed (7 vectors and byte-length error)"
"#;

#[test]
fn a_security_utils_comparison_runs_in_the_emitted_app() {
    app().run_ruby(STRING_COMPARISONS).assert_passes();
}

#[test]
fn a_fixed_length_security_utils_comparison_runs_in_the_emitted_app() {
    app().run_ruby(FIXED_LENGTH_COMPARISONS).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn a_security_utils_comparison_runs_natively() {
    app().run_spinel(STRING_COMPARISONS).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn a_fixed_length_security_utils_comparison_runs_natively() {
    app().run_spinel(FIXED_LENGTH_COMPARISONS).assert_passes();
}

#[test]
fn secure_compare_rejects_ordinary_non_string_arguments() {
    app()
        .run_ruby(
            r#"
[nil, 1, :abc, [], {}, true].each do |invalid|
  [[invalid, "abc"], ["abc", invalid]].each do |left, right|
    begin
      Signatures.match(left, right)
    rescue NoMethodError
      next
    end
    raise "non-string argument was accepted"
  end
end
puts "SecurityUtils rejects ordinary non-string arguments (12 cases)"
"#,
        )
        .assert_passes();
}

#[test]
fn ruby_family_targets_ship_and_require_security_utils() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        let (emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        let signature = if target == BuildTarget::Spinel {
            "runtime/security_utils.rbs"
        } else {
            "sig/runtime/security_utils.rbs"
        };
        for path in ["runtime/security_utils.rb", signature] {
            assert!(emitted.join(path).is_file(), "{target:?} omitted {path}");
        }
        let source = std::fs::read_to_string(emitted.join("app/models/signatures.rb"))
            .expect("emitted service");
        assert!(
            source.contains("runtime/security_utils"),
            "{target:?}: {source}"
        );
    }
}
