//! A key into a plain `Hash[String, _]` is emitted as written.
//!
//! The signature says `Hash[String, untyped]`, but at run time the Hash
//! is whatever the caller passed. Indexing it with a Symbol key misses
//! the String entry natively. Coercing the key with `.to_s` (the params
//! store's indifferent access) read `"x"`'s value for `:x`.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONFIG: &str = "Rails.application.configure do\n  config.eager_load = false\nend\n";

const LOOKUP: &str = r#"module Lookup
  #: (Hash[String, untyped], Array[untyped]) -> Array[untyped]
  def self.pick(values, keys)
    keys.map { |key| values[key] }
  end
end
"#;

#[test]
fn a_symbol_key_into_a_string_typed_hash_reads_its_own_entry() {
    let run = emit_and_run::empty_app()
        .write("config/environments/development.rb", CONFIG)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("lib/lookup.rb", LOOKUP)
        .run_ruby("p Lookup.pick({ \"x\" => 1, x: 2 }, [\"x\", :x, :y])\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[1, 2, nil]\n");
}

/// A request's params value is String-keyed at run time whatever the
/// key's static type, so an untyped key that holds a Symbol still reads
/// its entry there: the narrowing above is for a plain Hash only.
#[test]
fn an_untyped_symbol_key_into_params_still_reads_the_string_entry() {
    let run = emit_and_run::empty_app()
        .write("config/environments/development.rb", CONFIG)
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  def index
    head :ok
  end

  private

  def read(name)
    params[name]
  end
end
"#,
        )
        .run_ruby(
            r#"
require_relative "app/controllers/widgets_controller"

controller = WidgetsController.new
controller.params = { "label" => "hit" }
p [controller.send(:read, :label), controller.send(:read, "label"), controller.send(:read, :other)]
"#,
        );
    run.assert_passes();
    assert_eq!(run.stdout, "[\"hit\", \"hit\", nil]\n");
}
