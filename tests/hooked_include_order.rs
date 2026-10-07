//! Emitted-program regression: an app module's `self.included(base)`
//! hook that calls the including class's own class methods runs after
//! them, as the source wrote it.
//!
//! A class defines `ATTRS` and `def self.attrs` and THEN includes
//! `Configurable`, whose hook does `base.send(:attr_reader, *base.attrs)`.
//! The Ruby emitter wrote every include first in the class body, so loading
//! the file raised `undefined method 'attrs'`. The expected output is what
//! CRuby prints for the same source.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const HOOK_PROBE: &str = r##"module HookProbe
  module Configurable
    def self.included(base)
      base.send(:attr_writer, :modified)
      base.send(:attr_reader, *base.attrs)
      base.attrs.each do |attr|
        base.send(:define_method, "#{attr}=") do |value|
          instance_variable_set("@#{attr}", value)
        end
      end
    end

    def to_h
      self.class.attrs.each_with_object({}) { |key, hash| hash[key] = send(key) }
    end
  end

  class Config
    ATTRS = [:alpha, :beta].freeze
    def self.attrs
      ATTRS
    end

    include Configurable

    DEFAULT = { alpha: "x" }.freeze

    def initialize(hash)
      @alpha = nil
      @beta = nil
      hash.each { |k, v| send("#{k}=", v) }
    end

    def flag?
      false
    end
  end

  def self.report
    c = Config.new(Config::DEFAULT)
    c.beta = "y"
    [c.to_h, c.flag?]
  end
end
"##;

#[test]
fn an_included_hook_reading_class_methods_runs_after_them() {
    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/hook_probe.rb", HOOK_PROBE)
        .run_ruby("p HookProbe.report\n");
    run.assert_passes();
    // CRuby on the source above.
    assert_eq!(run.stdout.trim_end(), r#"[{alpha: "x", beta: "y"}, false]"#);
    let config = std::fs::read_to_string(run.emitted.join("app/models/hook_probe/config.rb"))
        .expect("emitted config.rb");
    let attrs = config.find("def self.attrs").expect("attrs def");
    let include = config.find("include HookProbe::Configurable").expect("include");
    assert!(attrs < include, "the include precedes the class method its hook calls:\n{config}");
}
