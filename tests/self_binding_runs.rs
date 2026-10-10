//! `#: self as T` and `T.bind(self, T)` type the sends after them and run
//! no code: the emitted Ruby keeps the sends and drops the binding, so a
//! module method whose `self` is bound to its includer still calls the
//! includer's readers.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LIB: &str = r#"module Greeting
  def greet
    #: self as Person
    "hello #{name}"
  end

  def shout
    T.bind(self, Person)
    name.upcase
  end
end

class Person
  include Greeting

  attr_reader :name

  def initialize(name)
    @name = name
  end
end
"#;

const SCRIPT: &str = r#"
require_relative "app/models/person"
person = Person.new("ada")
raise "bound implicit send lost: #{person.greet}" unless person.greet == "hello ada"
raise "bound T.bind send lost: #{person.shout}" unless person.shout == "ADA"
puts "self binding contract passed"
"#;

#[test]
fn a_self_binding_runs_as_the_sends_around_it() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/person.rb", LIB)
        .run_ruby(SCRIPT)
        .assert_passes();
}
