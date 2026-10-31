//! PostgreSQL: user agent version constant, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "PATH\n  remote: vendor/gems/cspish-1.0.0\n  specs:\n    cspish (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    useragent (0.16.11)\n\nDEPENDENCIES\n  cspish!\n  useragent\n";

const USERAGENT_RBI: &str = r#"# typed: true
class UserAgent
  class << self
    def parse(string); end
  end
end
class UserAgent::Version
  include ::Comparable
  def initialize(str); end
  def <=>(other); end
  def to_s; end
  def to_str; end
  class << self
    def new(obj = T.unsafe(nil)); end
  end
end
"#;

/// secure_headers' shape: the gem's file requires useragent, and the class
/// body builds the floors it compares a browser's version against.
const CSPISH: &str = r#"require "useragent"

module Cspish
  class Policy
    VERSION_46 = ::UserAgent::Version.new("46")
    VERSION_10 = UserAgent::Version.new("10")
    FALLBACK_VERSION = ::UserAgent::Version.new("0")

    def self.modern?(version)
      UserAgent::Version.new(version) >= VERSION_46
    end

    def self.floors
      [VERSION_46.to_s, VERSION_10.to_a, FALLBACK_VERSION.to_str]
    end
  end
end
"#;

fn app(rbi: bool, postgresql: bool) -> emit_and_run::Overlay {
    let yml = if postgresql {
        "test_paths:\n  - test\n"
    } else {
        "test_paths:\n  - test\n"
    };
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", yml)
        .write("Gemfile.lock", LOCK)
        .write("vendor/gems/cspish-1.0.0/lib/cspish.rb", "require \"cspish/policy\"\n")
        .write("vendor/gems/cspish-1.0.0/lib/cspish/policy.rb", CSPISH)
        .write("lib/oracle.rb", "require \"cspish\"\n\nclass Oracle\n  def ask(version)\n    Cspish::Policy.modern?(version)\n  end\nend\n");
    if postgresql {
        app = app.write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_user_agent_version_constant\n");
    }
    if rbi {
        app = app.write("sorbet/rbi/gems/useragent@0.16.11.rbi", USERAGENT_RBI);
    }
    app
}

fn policy(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("app/models/cspish/policy.rb")).expect("cspish/policy.rb")
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_class_body_versions_compile_and_answer_like_the_gem_on_the_postgresql_tree() {
    for rbi in [false, true] {
        let (dir, errors) = app(rbi, true).emit(BuildTarget::Ruby);
        // `check` names the PostgreSQL adapter as having no runtime of its
        // own in target-independent analysis; the CRuby tree carries one.
        let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
        assert!(errors.is_empty(), "rbi={rbi}: {errors:#?}");
        let policy = policy(&dir);
        assert!(!policy.contains("roundhouse:"), "rbi={rbi}: {policy}");
        assert!(policy.contains("VERSION_46 = ::UserAgent::Version.new(\"46\")"), "rbi={rbi}: {policy}");
        assert!(policy.contains("UserAgent::Version.new(version) >= Cspish::Policy::VERSION_46"), "rbi={rbi}: {policy}");
        assert!(dir.join("runtime/user_agent_version.rb").exists(), "rbi={rbi}");

        // The class body runs where the emitted model loads it: the tree's
        // user_agent port (which requires its Version), then the file.
        let out = emit_and_run::ruby()
            .current_dir(&*dir)
            .args([
                "-W:no-deprecated",
                "-e",
                "require_relative 'runtime/user_agent'\n\
                 require_relative 'app/models/cspish/policy'\n\
                 p [Cspish::Policy.modern?('126.0'), Cspish::Policy.modern?('45.9'), Cspish::Policy::VERSION_46.class.name]\n\
                 p Cspish::Policy.floors\n\
                 p UserAgent::Version.new('126.0') >= Cspish::Policy::VERSION_46\n",
            ])
            .output()
            .expect("ruby");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "rbi={rbi}: {stdout}\n{stderr}");
        assert_eq!(
            stdout,
            "[true, false, \"UserAgent::Version\"]\n[\"46\", [10], \"0\"]\ntrue\n",
            "rbi={rbi}: {stderr}"
        );
    }
}

/// The installed useragent 0.16.11 answers the same (skipped when the gem
/// is not on the default GEM_PATH; tests/user_agent_version.rs holds the
/// port to the gem observation by observation).
#[test]
fn the_installed_gem_answers_the_same_comparisons() {
    let probe = emit_and_run::ruby().args(["-e", "gem 'useragent', '0.16.11'"]).output().expect("ruby");
    if !probe.status.success() {
        eprintln!("useragent 0.16.11 is not installed; skipping the native control");
        return;
    }
    let out = emit_and_run::ruby()
        .args([
            "-W:no-deprecated",
            "-e",
            "gem 'useragent', '0.16.11'\nrequire 'user_agent'\n\
             v46 = ::UserAgent::Version.new('46')\n\
             p [UserAgent::Version.new('126.0') >= v46, UserAgent::Version.new('45.9') >= v46, v46.class.name]\n\
             p [v46.to_s, UserAgent::Version.new('10').to_a, UserAgent::Version.new('0').to_str]\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout, "[true, false, \"UserAgent::Version\"]\n[\"46\", [10], \"0\"]\n");
}

/// A SQLite CRuby tree and a Spinel tree do not carry the Version file, so
/// they refuse the read by name rather than emit a constant that raises
/// NameError at load.
#[test]
#[ignore = "fork spec: postgresql"]
fn trees_without_the_version_file_refuse_the_read() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (dir, errors) = app(false, false).emit(target);
        assert!(!dir.join("runtime/user_agent_version.rb").exists(), "{target:?}");
        assert!(
            errors.iter().any(|e| e.contains("UserAgent::Version is declared by the Ruby-family runtime (runtime/user_agent_version.rb)")),
            "{target:?}: {errors:#?}"
        );
    }
}
