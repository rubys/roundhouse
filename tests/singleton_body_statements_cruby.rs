//! Statements in a gem's `class << self` body run on the singleton class,
//! as in CRuby.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SOURCE: &str = r##"module SingletonWriters
  class Config
    attr_accessor :host, :secret
  end

  class << self
    attr_writer :config

    def config
      @config ||= Config.new
    end

    WRITERS = ["host", "secret"]

    WRITERS.each do |attr|
      define_method("#{attr}=") do |value|
        SingletonWriters.config.send("#{attr}=", value)
      end
    end

    READERS = WRITERS + ["kind"]

    delegate(*WRITERS, to: :config)

    self.define_method(:readers) { READERS }

    def kind
      "singleton"
    end
  end
end
"##;

const SCRIPT: &str = r#"SingletonWriters.host = "h"
SingletonWriters.secret = "s"
p [SingletonWriters.host, SingletonWriters.secret, SingletonWriters.readers, SingletonWriters.kind]
p [SingletonWriters.constants.sort, SingletonWriters.singleton_class.constants(false).sort, (SingletonWriters::WRITERS rescue :name_error)]
p [SingletonWriters.respond_to?(:host=), SingletonWriters.instance_methods.include?(:host=)]
"#;

const EXPECTED: &str = "[\"h\", \"s\", [\"host\", \"secret\", \"kind\"], \"singleton\"]\n\
[[:Config], [:READERS, :WRITERS], :name_error]\n\
[true, false]\n";

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn singleton_body_statements_on_a_constant_run_on_the_singleton_class_as_in_cruby() {
    let native = emit_and_run::ruby().arg("-e").arg(format!("require \"active_support/core_ext/module/delegation\"\n{SOURCE}\n{SCRIPT}")).output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), EXPECTED, "native control");

    let run = emit_and_run::real_blog().write("app/services/singleton_writers.rb", SOURCE).run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED, "{}", run.stderr);
}

const GEM_LOCK: &str = "PATH\n  remote: vendor/gems/writers_gem-1.0.0\n  specs:\n    writers_gem (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  writers_gem!\n";

/// token_client's shape: the module's entry file requires the file
/// whose `class << self` holds the constant and the loop, and the gem is
/// compiled as a source (`gem_sources:`), whose class bodies replay.
const GEM_FILES: &[(&str, &str)] = &[
    ("lib/writers_gem.rb", "require \"active_support/core_ext/module/delegation\"\n\nmodule WritersGem\n  VERSION = \"1.0.0\"\nend\n\nrequire_relative \"writers_gem/global_config\"\n"),
    (
        "lib/writers_gem/global_config.rb",
        "module WritersGem\n  class Config\n    attr_accessor :host, :secret\n  end\n\n  class << self\n    def config\n      @config ||= Config.new\n    end\n\n    WRITERS = [\"host\", \"secret\"]\n\n    WRITERS.each do |attr|\n      define_method(\"#{attr}=\") do |value|\n        WritersGem.config.send(\"#{attr}=\", value)\n      end\n    end\n\n    READERS = WRITERS + [\"kind\"]\n\n    delegate(*WRITERS, to: :config)\n\n    def kind\n      \"gem\"\n    end\n  end\nend\n",
    ),
];

const GEM_SCRIPT: &str = r#"WritersGem.host = "h"
WritersGem.secret = "s"
p [WritersGem.host, WritersGem.secret, WritersGem.kind, WritersGem.singleton_class::READERS]
p [WritersGem.singleton_class.constants(false).sort, (WritersGem::WRITERS rescue :name_error)]
"#;

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_gem_source_singleton_body_loop_runs_on_the_singleton_class_as_in_cruby() {
    let gem = std::env::temp_dir().join(format!("rh_writers_gem_native_{}", std::process::id()));
    for (path, text) in GEM_FILES {
        let file = gem.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let native = emit_and_run::ruby().arg("-I").arg(gem.join("lib")).args(["-rwriters_gem", "-e", GEM_SCRIPT]).output().expect("CRuby control");
    std::fs::remove_dir_all(&gem).unwrap();
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = "[\"h\", \"s\", \"gem\", [\"host\", \"secret\", \"kind\"]]\n[[:READERS, :WRITERS], :name_error]\n";
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected, "native control");

    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", GEM_LOCK)
        .write("config/application.rb", "require \"writers_gem\"\n");
    for (path, text) in GEM_FILES {
        app = app.write(&format!("vendor/gems/writers_gem-1.0.0/{path}"), text);
    }
    let run = app.run_ruby(GEM_SCRIPT);
    assert!(run.success, "{}\n{}\n{}\n{:?}", run.emitted.display(), run.stdout, run.stderr, run.errors);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
}
