//! Caller-visible execution of the supported ActiveSupport::Inflector slice.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ASSERTIONS: &str = r#"
inflector = ActiveSupport::Inflector
raise "camelize" unless inflector.camelize("foo_bar") == "FooBar"
raise "camelize word normalization" unless inflector.camelize("foo_BAR") == "FooBar"
raise "camelize punctuation boundary" unless inflector.camelize("foo_BAR-Baz") == "FooBar-Baz"
raise "lower camelize" unless inflector.camelize("foo/bar_baz", false) == "foo::BarBaz"
raise "namespace camelize" unless inflector.camelize("/foo/bar") == "::Foo::Bar"
raise "deconstantize" unless inflector.deconstantize("Admin::UsersController") == "Admin"
raise "top-level deconstantize" unless inflector.deconstantize("UsersController").empty?
raise "foreign_key" unless inflector.foreign_key("Admin::APIKey") == "api_key_id"
raise "foreign_key option" unless inflector.foreign_key("Admin::APIKey", false) == "api_keyid"
raise "upcase_first" unless inflector.upcase_first("hello") == "Hello"
raise "downcase_first" unless inflector.downcase_first("WORLD") == "wORLD"
puts "ActiveSupport::Inflector caller contract passed"
"#;

#[test]
fn inflector_slice_runs_in_emitted_cruby_app() {
    let run = emit_and_run::real_blog().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run
        .stdout
        .contains("ActiveSupport::Inflector caller contract passed"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn inflector_slice_runs_in_native_spinel_app() {
    let run = emit_and_run::real_blog().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run
        .stdout
        .contains("ActiveSupport::Inflector caller contract passed"));
}
