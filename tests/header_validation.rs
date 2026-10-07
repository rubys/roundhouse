//! The portable predicates and CRuby/JRuby optimization share one HTTP policy.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn fast_header_predicates_match_the_portable_policy() {
    let script = r#"
require_relative "runtime/ruby/action_controller/base"
shared_key = ActionController.method(:header_key_ok?)
shared_value = ActionController.method(:header_value_ok?)
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/header_validation_cruby"
checks = [[nil, false, false], ["", false, true], ["Café", true, true],
          ["X:\u0301", false, true], ["X \u0301", false, true],
          ["X\"\u0301", false, true], ["X-e\u0301", true, true],
          ["X-\xFF".force_encoding("UTF-8"), true, true],
          ["X:\xFF".force_encoding("UTF-8"), false, true],
          ["a\xFF\n".force_encoding("UTF-8"), false, false],
          ["X-\xFF".b, true, true], ["X:\xFF".b, false, true],
          ["a\xFF\n".b, false, false]]
128.times do |byte|
  text = "a" + byte.chr + "b"
  checks << [text, byte > 32 && byte != 127 && byte != 34 && byte != 58,
             (byte >= 32 || byte == 9) && byte != 127]
end
checks.each do |text, key_ok, value_ok|
  raise "shared key #{text.inspect}" unless shared_key.call(text) == key_ok
  raise "shared value #{text.inspect}" unless shared_value.call(text) == value_ok
  raise "fast key #{text.inspect}" unless ActionController.header_key_ok?(text) == key_ok
  raise "fast value #{text.inspect}" unless ActionController.header_value_ok?(text) == value_ok
end
puts "ALL OK"
"#;
    let output = std::process::Command::new("ruby").args(["-e", script])
        .current_dir(env!("CARGO_MANIFEST_DIR")).output().expect("ruby available");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("ALL OK"));
}

#[test]
fn emitted_ruby_boot_loads_the_fast_header_predicates() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n create_table :widgets do |t|\n  t.string :name\n end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", "class WidgetsController < ApplicationController\n def show\n  render plain: 'ok'\n end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n get '/ok', to: 'widgets#show'\nend\n")
        .run_ruby(r#"
[:header_key_ok?, :header_value_ok?].each do |name|
  file, = ActionController.method(name).source_location
  raise "portable fallback loaded for #{name}: #{file}" unless file.end_with?("/runtime/header_validation_cruby.rb")
end
raise "invalid key accepted" if ActionController.header_key_ok?("Bad:Key")
raise "tab rejected" unless ActionController.header_value_ok?("a\tb")
raise "newline accepted" if ActionController.header_value_ok?("a\nb")
puts "ALL OK"
"#).assert_passes();
}

/// Swift must compile the shared predicates and reject a CRLF grapheme.
/// Module helpers are internal APIs; this SDK-specific fixture does not claim
/// that every target exports them for direct application/test imports.
#[test]
#[ignore = "requires Swift 6+, SQLite development headers and SPM dependencies"]
fn header_predicates_execute_on_swift() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n create_table :widgets do |t|\n  t.string :name\n end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", "class WidgetsController < ApplicationController\n def show\n  render plain: 'ok'\n end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n get '/ok', to: 'widgets#show'\nend\n")
        .write("test/controllers/header_policy_test.rb", r#"class HeaderPolicyTest < Minitest::Test
  def test_header_key_policy_rejects_controls_and_delimiters
    refute ActionController.header_key_ok?(nil)
    refute ActionController.header_key_ok?("")
    refute ActionController.header_key_ok?("X:Bad")
    refute ActionController.header_key_ok?("X Bad")
    refute ActionController.header_key_ok?("X\"Bad")
    refute ActionController.header_key_ok?("X\tBad")
    refute ActionController.header_key_ok?("X\r\nBad")
    refute ActionController.header_key_ok?("X\x00Bad")
    refute ActionController.header_key_ok?("X\x1fBad")
    refute ActionController.header_key_ok?("X\x7fBad")
    refute ActionController.header_key_ok?("X:\u0301")
    refute ActionController.header_key_ok?("X \u0301")
    refute ActionController.header_key_ok?("X\"\u0301")
    assert ActionController.header_key_ok?("X-e\u0301")
    assert ActionController.header_key_ok?("X-Valid")
    assert ActionController.header_key_ok?("X-Café")
  end

  def test_header_value_policy_allows_tab_but_rejects_other_controls
    refute ActionController.header_value_ok?(nil)
    refute ActionController.header_value_ok?("a\r\nb")
    refute ActionController.header_value_ok?("a\x00b")
    refute ActionController.header_value_ok?("a\x08b")
    refute ActionController.header_value_ok?("a\x0ab")
    refute ActionController.header_value_ok?("a\x1fb")
    refute ActionController.header_value_ok?("a\x7fb")
    assert ActionController.header_value_ok?("")
    assert ActionController.header_value_ok?("a\tb")
    assert ActionController.header_value_ok?(" : Café ")
  end

end
"#)
        .emit(roundhouse::project::BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let output = std::process::Command::new("swift")
        .args(["test", "--jobs", "2", "--filter", "HeaderPolicyTest"])
        .current_dir(&emitted).output().expect("run generated Swift header contract");
    std::fs::write(emitted.join("swift-test.stdout"), &output.stdout).unwrap();
    std::fs::write(emitted.join("swift-test.stderr"), &output.stderr).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "Swift header contract at {}:\n{stdout}\n{stderr}", emitted.display());
    assert!(stdout.contains("Executed 2 tests, with 0 failures"), "Swift header cases did not both execute:\n{stdout}");
}
