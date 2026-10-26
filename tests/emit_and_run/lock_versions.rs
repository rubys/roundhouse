//! Framework and gem versions, answered from the app's Gemfile.lock, and
//! `Gem::Version` values. flag_shih_tzu branches on
//! `ActiveRecord::VERSION::MAJOR`; counter_culture compares
//! `Gem.loaded_specs["activerecord"].version` with `Gem::Version.new("5.1.0")`;
//! discourse and chatwoot compare and sort `Gem::Version`s.

use super::emit_and_run;

const PROBE: &str = r#"class VersionProbe
  AR_VERSION = Gem.loaded_specs["activerecord"].version

  def self.facts
    [ActiveRecord::VERSION::STRING, ActiveRecord::VERSION::MAJOR, ::Rails::VERSION::MINOR, Rails::VERSION::PRE,
     Rails.version, ActiveRecord.gem_version.to_s, AR_VERSION.to_s]
  end

  def self.compares
    [AR_VERSION >= Gem::Version.new("5.1.0"), Gem::Version.new("1.10") > Gem::Version.new("1.9"),
     Gem::Version.new("2.0.rc1").prerelease?, Gem::Version.new("1.2") <=> Gem::Version.new("1.2.0"),
     %w[1.10 1.9 1.2.a].sort_by { |v| Gem::Version.new(v) }]
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/version_probe.rb", PROBE)
}

fn locked(gem: &str) -> String {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/real-blog/Gemfile.lock")).unwrap();
    let line = lock.lines().find(|l| l.starts_with(&format!("    {gem} ("))).unwrap();
    line.trim().trim_start_matches(gem).trim().trim_matches(|c| c == '(' || c == ')').to_string()
}

fn assertions() -> String {
    let ar = locked("activerecord");
    let rails = locked("railties");
    let parts: Vec<&str> = rails.split('.').collect();
    let pre = parts.get(3).map_or("nil".to_string(), |p| format!("{p:?}"));
    format!(
        r#"got = [VersionProbe.facts, VersionProbe.compares]
want = [[{ar:?}, {ar_major}, {rails_minor}, {pre}, {rails:?}, {ar:?}, {ar:?}],
        [true, true, true, 0, ["1.2.a", "1.9", "1.10"]]]
raise got.inspect unless got == want
puts "lock versions passed"
"#,
        ar_major = ar.split('.').next().unwrap(),
        rails_minor = parts[1],
    )
}

#[test]
fn lock_versions_run() {
    let run = app().run_ruby(&assertions());
    run.assert_passes();
    assert!(run.stdout.contains("lock versions passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn lock_versions_run_on_spinel() {
    let run = app().run_spinel(&assertions());
    run.assert_passes();
    assert!(run.stdout.contains("lock versions passed"), "{}", run.stdout);
}

/// No strict runtime has a `Gem::Version`; the folded versions themselves are plain literals there.
#[test]
fn gem_version_is_unsupported_on_strict_targets() {
    use roundhouse::project::BuildTarget::*;
    for target in [Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Elixir, Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.iter().any(|e| e.contains("Gem::Version")), "{target:?}: {errors:#?}");
        assert!(!errors.iter().any(|e| e.contains("VERSION::")), "{target:?}: {errors:#?}");
        let (_emitted, errors) = emit_and_run::real_blog()
            .write("app/models/locked_probe.rb", "class LockedProbe\n  def self.ar\n    Gem.loaded_specs[\"activerecord\"].version\n  end\nend\n")
            .emit(target);
        assert!(errors.iter().any(|e| e.contains("Gem::Version")), "{target:?} folded: {errors:#?}");
    }
}
