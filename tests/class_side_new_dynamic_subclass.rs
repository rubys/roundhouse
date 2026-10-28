//! A class a gem builds at run time (`Class.new(Base)`) keeps the class
//! side its base gives it.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const MEASURED: &str = r#"module Measured
  class Measurable
    attr_reader :value

    def initialize(value)
      @value = value
    end

    def unit
      self.class.unit
    end

    class << self
      def unit
        raise "`Measurable` does not have a unit"
      end

      def parse(string)
        new(string.to_i)
      end
    end
  end

  class << self
    def build(unit)
      Class.new(Measurable) do
        define_singleton_method(:unit) { unit }
      end
    end
  end

  Weight = build("lb")
end
"#;

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_run_time_subclass_parses_to_itself() {
    let run = emit_and_run::real_blog().write("app/models/measured.rb", MEASURED).run_ruby(
        "w = Measured::Weight.parse(\"3\")\n\
         raise \"built #{w.class}\" unless w.instance_of?(Measured::Weight)\n\
         raise \"unit #{w.unit}\" unless w.unit == \"lb\" && w.value == 3\n\
         puts \"ok\"\n",
    );
    assert!(run.success && run.stdout.contains("ok"), "{}\n{}", run.stdout, run.stderr);
}

/// measured's gem shape: `class << self` forwards with Forwardable's hash
/// `delegate`, and the concrete class is a file-level path write whose
/// value is a call (`Meas::Weight = Meas.build do ... end`). Both were
/// dropped from the emitted gem: `unit_names` was undefined and
/// `Meas::Weight` an unknown constant.
#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_gem_built_class_keeps_its_forwarded_class_methods() {
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"activesupport\"\ngem \"meas\", \"~> 1.0.0\"\n")
        .write(
            "Gemfile.lock",
            "PATH\n  remote: vendor/gems/meas-1.0.0\n  specs:\n    meas (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  meas (~> 1.0.0)\n",
        )
        .write("config/boot.rb", "require \"bundler/setup\"\n")
        .write(
            "config/application.rb",
            "require_relative \"boot\"\n\nrequire \"rails\"\nrequire \"active_record/railtie\"\n\nBundler.require(*Rails.groups)\n\nmodule MeasProbe\n  class Application < Rails::Application\n  end\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("config/environment.rb", "require_relative \"application\"\n\nRails.application.initialize!\n")
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write(
            "vendor/gems/meas-1.0.0/lib/meas.rb",
            "require \"forwardable\"\n\nmodule Meas\n  class << self\n    def build(unit, &block)\n      Class.new(Measurable) do\n        define_singleton_method(:unit_system) { UnitSystem.new([unit]) }\n      end\n    end\n  end\nend\n\nrequire \"meas/unit_system\"\nrequire \"meas/measurable\"\nrequire \"meas/weight\"\n",
        )
        .write(
            "vendor/gems/meas-1.0.0/lib/meas/unit_system.rb",
            "class Meas::UnitSystem\n  attr_reader :unit_names\n\n  def initialize(unit_names)\n    @unit_names = unit_names\n  end\nend\n",
        )
        .write(
            "vendor/gems/meas-1.0.0/lib/meas/measurable.rb",
            "class Meas::Measurable\n  attr_reader :value\n\n  def initialize(value)\n    @value = value\n  end\n\n  class << self\n    extend Forwardable\n\n    def unit_system\n      raise \"`Measurable` does not have a unit system\"\n    end\n\n    delegate unit_names: :unit_system\n  end\nend\n",
        )
        .write("vendor/gems/meas-1.0.0/lib/meas/weight.rb", "Meas::Weight = Meas.build(\"lb\") do\nend\n")
        .write(
            "test/models/weight_test.rb",
            "require \"test_helper\"\n\nclass WeightTest < ActiveSupport::TestCase\n  test \"built weight\" do\n    assert_equal [\"lb\"], Meas::Weight.unit_names\n    assert_equal 3, Meas::Weight.new(3).value\n  end\nend\n",
        )
        .run_test("test/models/weight_test.rb");
    // `unit_system`'s value is built at run time, so the forwarder's
    // destination stays unverified (a diagnostic, written as `...`).
    assert!(
        !run.errors.iter().any(|e| e.contains("not carried") || e.contains("unknown constant") || e.contains("gem_requires_rails")),
        "{:#?}",
        run.errors
    );
    assert!(run.success && run.stdout.contains("1 tests passed"), "{}\n{}", run.stdout, run.stderr);
}
