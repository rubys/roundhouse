//! Ruby and Spinel supply these constants; resolving source names must
//! preserve the constructors and rescue classes in emitted Ruby.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn struct_mutex_and_json_error_constants_execute_after_emission() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/registry.rb", r#"require "json"
class Registry
  Entry = Struct.new(:lock, :members)
  def self.entry
    Entry.new(Mutex.new, {})
  end
  def self.parse(text)
    JSON.parse(text)
  rescue JSON::ParserError
    nil
  end
end
"#)
        .run_ruby(r#"
entry = Registry.entry
entry.lock.synchronize { entry.members["member"] = 7 }
raise "struct or mutex lost" unless entry.members == { "member" => 7 }
raise "valid JSON changed" unless Registry.parse('{"ok":true}') == { "ok" => true }
raise "JSON rescue lost" unless Registry.parse('{') == nil
puts "stdlib constant execution passed"
"#)
        .assert_passes();
}
