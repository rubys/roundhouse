//! `Rails.cache.read/write/delete/exist?` take Rails' trailing options
//! Hash — `write(name, value, options = nil)` — so an app's
//! `Rails.cache.write(k, v, expires_in: 30.minutes)` reaches the store
//! instead of raising `ArgumentError` on arity.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LIB: &str = r#"class Tokens
  def self.remember(key, value)
    Rails.cache.write(key, value, expires_in: 30.minutes)
    Rails.cache.read(key, raw: true)
  end

  def self.present?(key)
    Rails.cache.exist?(key, namespace: "tokens")
  end

  def self.forget(key)
    Rails.cache.delete(key, namespace: "tokens")
  end
end
"#;

#[test]
fn cache_entry_calls_accept_an_options_hash() {
    let run = emit_and_run::real_blog()
        .write("app/services/tokens.rb", LIB)
        .run_ruby(
            r#"Tokens.remember("widget:1", "abc")
Tokens.present?("widget:1")
Tokens.forget("widget:1")
puts "cache options passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("cache options passed"));
}
