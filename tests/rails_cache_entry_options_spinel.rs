//! The Spinel side of `tests/rails_cache_entry_options.rs`: the shared
//! runtime's `Rails::Cache` (`runtime/ruby/rails.rb`) takes Rails'
//! trailing options Hash on `read`/`write`/`delete`/`exist?`, so
//! `Rails.cache.write(k, v, expires_in: 30.minutes)` compiles and runs
//! instead of raising on arity. Needs a Spinel compiler, so `#[ignore]`d
//! like its sibling suites; CI's `spinel-framework` job runs it with
//! `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

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
#[ignore = "requires the Spinel toolchain"]
fn cache_entry_calls_accept_an_options_hash_on_spinel() {
    let run = emit_and_run::real_blog()
        .write("app/services/tokens.rb", LIB)
        .run_spinel(
            r#"Tokens.remember("widget:1", "abc")
Tokens.present?("widget:1")
Tokens.forget("widget:1")
puts "cache options passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("cache options passed"));
}
