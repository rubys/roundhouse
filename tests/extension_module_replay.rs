//! A plain class's body calls into an app module it extends.
//!
//! `extend SymbolEnum` followed by `symbol_enum :state, [:on, :off]` in
//! a class with no superclass: no runtime base answers either call, so
//! the app's own module is the receiver's, and the emitted body has to
//! run both in source order. They were dropped as "not modelled", and
//! the class then lacked everything the macro defines.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const REGISTRY: &str = r#"module Registry
  #: (Symbol name) -> Array[Symbol]
  def register(name)
    registered << name
  end

  #: () -> Array[Symbol]
  def registered
    @registered ||= [] #: Array[Symbol]?
  end
end
"#;

const WIDGET: &str = r#"class Widget
  extend Registry

  register :a
  register :b

  #: () -> Array[Symbol]
  def self.report
    registered
  end
end
"#;

#[test]
fn an_extended_app_module_s_macros_run_in_the_class_body() {
    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/registry.rb", REGISTRY)
        .write("app/models/widget.rb", WIDGET)
        .run_ruby("p Widget.report\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[:a, :b]\n");
}
