//! Ruby calls `inherited(subclass)` with the new Class, so the hook's
//! argument is a class object: `def self.inherited(sub)` on a class, or a
//! `def inherited(sub)` in a module the class extends, may run Module
//! protocol (`class_eval`) on it. Refused as an unproven receiver, the
//! replayed hook raised when each subclass was defined.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const REGISTRY: &str = r#"module Registry
  def inherited(subclass)
    super
    subclass.class_eval do
      @registered = true
    end
  end

  def registered?
    @registered == true
  end
end
"#;

const BASE: &str = r#"class Base
  def self.inherited(sub)
    super
    sub.class_eval { @seen = true }
  end

  def self.seen?
    @seen == true
  end
end

class Child < Base
end
"#;

const EXTENDED: &str = r#"class Node
  extend Registry
end

class Leaf < Node
end
"#;

const SCRIPT: &str = r#"
require_relative "app/models/base"
raise "hook on the class did not run" unless Child.seen?
raise "hook ran on the base" if Base.seen?
puts "inherited hook contract passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

#[test]
fn a_class_side_inherited_hook_receives_a_class() {
    app().write("app/services/base.rb", BASE).run_ruby(SCRIPT).assert_passes();
}

/// A module's `inherited` is the hook of the classes that extend it.
#[test]
fn an_extended_modules_inherited_hook_receives_a_class() {
    let (_, errors) = app()
        .write("app/services/registry.rb", REGISTRY)
        .write("app/services/node.rb", EXTENDED)
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
