//! An `included` hook that only calls a framework macro on its receiver
//! (`base.before_action(...) if base.methods.include?(:before_action)`)
//! doesn't change constructor lookup, so the including class keeps its
//! keyword construction (`tests/constructor_unrelated_mutations.rs` has
//! the analyzer side). Emitted and run, as every newly accepted shape is.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_hook_that_only_calls_a_macro_on_its_receiver_runs_keyword_construction() {
    emit_and_run::empty_app()
        .write(
            "app/models/tagging.rb",
            "module Tagging\n  def self.included(base)\n    base.before_action(:load_tag) if base.methods.include?(:before_action)\n  end\nend\n",
        )
        .write(
            "app/models/widget_client.rb",
            "class WidgetClient\n  include Tagging\n  attr_reader :name\n\n  def initialize(args)\n    @name = args[:name]\n  end\n\n  def self.build\n    new(name: \"x\")\n  end\nend\n",
        )
        .run_ruby("raise \"build\" unless WidgetClient.build.name == \"x\"\nputs \"PASS\"\n")
        .assert_passes();
}
