//! One ERB contract shared by the interpreted and native lanes: an app
//! renders a template string with `ERB.new(template, trim_mode: "-")
//! .result_with_hash(hash)` and escapes with `ERB::Util.html_escape`.
//! The expected values are what CRuby's ERB answers.
//!
//! On spinel the emitted runtime's ERB shim defines `ERB`, so the bundled
//! `require "erb"` row is skipped for the whole tree; while the shim was a
//! `module ERB` holding only `Util`, spinel's `packages/erb` never loaded
//! and `ERB.new` raised "undefined method 'new' for module ERB".

pub const SOURCE: &str = r#"require "erb"
class Letter
  TEMPLATE = "Dear <%= name %>,\n<%# greeting -%>\nTotal: <%= total %>\n"

  def self.render(name, total)
    ERB.new(TEMPLATE, trim_mode: "-").result_with_hash(name: name, total: total)
  end

  def self.escape(text)
    ERB::Util.html_escape(text)
  end
end
"#;

pub const ASSERTIONS: &str = r#"
letter = Letter.render("Ada", "3")
raise "result_with_hash: #{letter.inspect}" unless letter == "Dear Ada,\nTotal: 3\n"
escaped = Letter.escape("<b>&")
raise "html_escape: #{escaped.inspect}" unless escaped == "&lt;b&gt;&amp;"
raise "ERB is a class" unless ERB.is_a?(Class)
puts "erb contract passed"
"#;

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/letter.rb", SOURCE)
}
