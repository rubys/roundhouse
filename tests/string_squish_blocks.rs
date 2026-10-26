#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n self.abstract_class = true\nend\n")
        .write("app/models/probe_entry.rb", r#"class ProbeEntry < ApplicationRecord
  normalizes :name, with: ->(name) { name.squish }
  attr_accessor :search_terms

  def squished_text
    text.squish
  end

  def squished_terms
    Array(search_terms).map { |term| term.squish.downcase }.join("|")
  end
end
"#)
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n create_table :probe_entries do |t|\n  t.string :name, null: false\n  t.text :text, null: false\n end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/probes_controller.rb", "class ProbesController < ApplicationController\n def index\n  entry = ProbeEntry.new(name: \" a b \", text: \" c d \")\n  entry.search_terms = [\" e f \"]\n  render json: {name: entry.name, text: entry.squished_text, terms: entry.squished_terms}\n end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n root \"probes#index\"\nend\n")
}

#[test]
fn synthesized_string_normalizer_uses_the_shared_squish_helper() {
    let (tree, _, errors) = app().emit_with_app(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let source = std::fs::read_to_string(tree.join("app/models/probe_entry.rb")).unwrap();
    let normalizer = source
        .split("def self._normalize_name(value)")
        .nth(1)
        .expect("normalizer emitted")
        .split("\n  end")
        .next()
        .unwrap();
    assert!(
        // The value is cast by the String column's type first, into a
        // String local.
        normalizer.contains("ActiveSupport.squish(cast_value)"),
        "{normalizer}"
    );
    assert!(source.contains("term.squish.downcase"), "{source}");
}

#[test]
fn normalized_assignment_matches_rails_unicode_empty_and_nil_cases() {
    app()
        .run_ruby(
            r#"
entry = ProbeEntry.new(name: " \tAlpha\u00a0 Beta\n ", text: " \nHello\u2003 World\t ")
raise entry.name.inspect unless entry.name == "Alpha Beta"
raise entry.squished_text.inspect unless entry.squished_text == "Hello World"
entry.name = " \t\n "
raise entry.name.inspect unless entry.name == ""
entry.name = nil
raise entry.name.inspect unless entry.name.nil?
puts "String normalization matches Rails"
"#,
        )
        .assert_passes();
}

#[test]
fn explicitly_typed_accessor_terms_match_rails_squish_results() {
    app()
        .write(
            "sig/probe_entry.rbs",
            "class ProbeEntry\n def search_terms: () -> Array[String]?\nend\n",
        )
        .run_ruby(
            r#"
entry = ProbeEntry.allocate
entry.search_terms = [" \tHELLO\u00a0 World ", "\nOther term\t"]
raise entry.squished_terms.inspect unless entry.squished_terms == "hello world|other term"
entry.search_terms = []
raise entry.squished_terms.inspect unless entry.squished_terms == ""
entry.search_terms = nil
raise entry.squished_terms.inspect unless entry.squished_terms == ""
puts "Declared String term squish matches Rails"
"#,
        )
        .assert_passes();
}
