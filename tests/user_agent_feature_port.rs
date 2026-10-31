//! PostgreSQL: user agent feature port, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

#[test]
#[ignore = "fork spec: postgresql layout"]
fn a_native_require_of_useragent_loads_the_port() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_user_agent_feature_port\n")
        .write("lib/oracle.rb", "class Oracle\nend\n")
        .emit(BuildTarget::Ruby);
    for feature in ["useragent", "user_agent", "platform_agent"] {
        let path = dir.join(format!("runtime/features/{feature}.rb"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(text.contains("require_relative \"../user_agent\""), "{text}");
    }

    let home = dir.join("tmp-gem-home");
    let lib = home.join("gems/useragent-0.16.11/lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("useragent.rb"), "require 'user_agent'\n").unwrap();
    std::fs::write(lib.join("user_agent.rb"), "class UserAgent\n  NATIVE = true\n  MATCHER = /native/\nend\n").unwrap();
    std::fs::create_dir_all(home.join("specifications")).unwrap();
    std::fs::write(
        home.join("specifications/useragent-0.16.11.gemspec"),
        "Gem::Specification.new do |s|\n  s.name = \"useragent\"\n  s.version = \"0.16.11\"\n  s.summary = \"x\"\n  s.authors = [\"x\"]\n  s.files = [\"lib/useragent.rb\", \"lib/user_agent.rb\"]\n  s.require_paths = [\"lib\"]\nend\n",
    )
    .unwrap();
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .env("RH_TEST_GEMS", &home)
        .args([
            "-e",
            // gem_facades.rb's order: the port, the features path, then
            // the gem source's dependency.
            "Gem.paths = { 'GEM_PATH' => [ENV['RH_TEST_GEMS'], *Gem.path].join(File::PATH_SEPARATOR) }\n\
             require_relative 'runtime/user_agent'\n\
             $LOAD_PATH.unshift(File.expand_path('runtime/features'))\n\
             require 'useragent'\n\
             require 'platform_agent'\n\
             puts defined?(UserAgent::NATIVE).inspect\n\
             puts UserAgent.parse('Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0').browser\n\
             puts $LOADED_FEATURES.grep(%r{/useragent-}).inspect\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "nil\nFirefox\n[]\n", "{stderr}");
    assert!(!stderr.contains("already initialized constant"), "{stderr}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_ported_user_agent_carries_the_gems_version() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_user_agent_version\n")
        .write("lib/oracle.rb", "class Oracle\nend\n")
        .emit(BuildTarget::Ruby);
    let port = std::fs::read_to_string(dir.join("runtime/user_agent.rb")).unwrap();
    assert!(port.ends_with("require_relative \"user_agent_version\"\n"), "{port}");
    let feature = std::fs::read_to_string(dir.join("runtime/features/user_agent/version.rb")).unwrap();
    assert!(feature.contains("require_relative \"../../user_agent_version\""), "{feature}");
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args([
            "-W:no-deprecated",
            "-e",
            "require_relative 'runtime/user_agent'\n\
             $LOAD_PATH.unshift(File.expand_path('runtime/features'))\n\
             require 'user_agent/version'\n\
             v46 = ::UserAgent::Version.new('46')\n\
             p [UserAgent::Version.new('126.0') >= v46, UserAgent::Version.new('45.9') >= v46, UserAgent::Version.new('4.0b2') < UserAgent::Version.new('4.0'), UserAgent::Version.new(nil).nil?]\n\
             p UserAgent.parse('Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0').browser\n\
             p $LOADED_FEATURES.grep(%r{useragent-|user_agent/version}).map { |f| File.basename(File.dirname(f)) + '/' + File.basename(f) }\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "[true, false, true, true]\n\"Firefox\"\n[\"user_agent/version.rb\"]\n", "{stderr}");
    assert!(!stderr.contains("already initialized constant") && !stderr.contains("method redefined"), "{stderr}");
}

/// The Version file is the gem's code (a `self.new` override, mixed
/// Integer/String segments), so only the PostgreSQL CRuby tree carries it:
/// a SQLite CRuby tree and a Spinel tree keep the typed port alone.
#[test]
fn only_the_postgresql_cruby_tree_carries_the_version_file() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (dir, _) = emit_and_run::empty_app()
            .write("lib/oracle.rb", "class Oracle\nend\n")
            .emit(target);
        assert!(!dir.join("runtime/user_agent_version.rb").exists(), "{target:?}");
        let port = std::fs::read_to_string(dir.join("runtime/user_agent.rb")).unwrap_or_default();
        assert!(!port.contains("user_agent_version"), "{target:?}");
    }
}
