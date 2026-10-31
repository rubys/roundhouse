//! PostgreSQL: activeresource base singleton cruby, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const GEM_LOCK: &str = "PATH\n  remote: vendor/gems/ts_gem-1.0.0\n  specs:\n    ts_gem (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  ts_gem!\n";

const GEM_FILES: &[(&str, &str)] = &[
    (
        "lib/ts_gem.rb",
        "require \"active_support\"\nrequire \"active_support/core_ext/class/attribute\"\nrequire_relative \"ts_gem/threadsafe_attributes\"\nrequire_relative \"ts_gem/base\"\n",
    ),
    (
        "lib/ts_gem/threadsafe_attributes.rb",
        r##"module ThreadsafeAttributes
  def self.included(klass)
    klass.extend(ClassMethods)
  end

  module ClassMethods
    def threadsafe_attribute(*attrs)
      attrs.each do |attr|
        define_method(attr) { instance_variable_get("@ts_#{attr}") }
        define_method("#{attr}=") { |value| instance_variable_set("@ts_#{attr}", value) }
        define_method("#{attr}_defined?") { instance_variable_defined?("@ts_#{attr}") }
      end
    end
  end
end
"##,
    ),
    (
        "lib/ts_gem/base.rb",
        r##"module TsGem
  class Base
    def self.logger=(logger)
      self._connection = nil
      @logger = logger
    end

    class_attribute :_format

    class << self
      include ThreadsafeAttributes
      threadsafe_attribute :_connection, :_site

      def site
        _site_defined? ? _site : "none"
      end

      def site=(value)
        self._connection = nil
        self._site = value
      end

      def format=(value)
        self._format = value
        self._connection = nil if _site_defined?
      end
    end

    self.format = :json
  end

  ActiveSupport.run_load_hooks(:ts_gem, Base)
end
"##,
    ),
];

const SCRIPT: &str = r#"class Billing < TsGem::Base
  self.format = :xml
  self.site = "https://b"
end
p [TsGem::Base.site, Billing.site, Billing._format, TsGem::Base._format]
p [TsGem::Base.singleton_class.include?(ThreadsafeAttributes), TsGem::Base.singleton_class.singleton_class.include?(ThreadsafeAttributes::ClassMethods)]
"#;

const EXPECTED: &str = "[\"none\", \"https://b\", :xml, :json]\n[true, true]\n";

fn app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("Gemfile.lock", GEM_LOCK)
        .write("config/application.rb", "require \"ts_gem\"\n");
    for (path, text) in GEM_FILES {
        app = app.write(&format!("vendor/gems/ts_gem-1.0.0/{path}"), text);
    }
    app
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_singleton_include_and_its_macro_run_on_the_singleton_class_as_in_cruby() {
    let gem = std::env::temp_dir().join(format!("rh_ts_gem_native_{}", std::process::id()));
    for (path, text) in GEM_FILES {
        let file = gem.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let native = emit_and_run::ruby().arg("-I").arg(gem.join("lib")).args(["-rts_gem", "-e", SCRIPT]).output().expect("CRuby control");
    std::fs::remove_dir_all(&gem).unwrap();
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), EXPECTED, "native control");

    let run = app().write("roundhouse.yml", "").run_ruby(SCRIPT);
    assert!(run.success, "{}\n{}\n{}\n{:?}", run.emitted.display(), run.stdout, run.stderr, run.errors);
    assert_eq!(run.stdout, EXPECTED, "{}", run.stderr);
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_relative_load_hook_base_resolves_against_its_namespace() {
    let (dir, errors) = app()
        .write(
            "roundhouse.yml",
            "",
        )
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_activeresource_base\n")
        .emit(BuildTarget::Ruby);
    let errors: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let hooks: Vec<String> = walk(&dir.join("app"))
        .into_iter()
        .flat_map(|text| text.lines().filter(|l| l.contains("run_load_hooks(:ts_gem")).map(|l| l.trim().to_string()).collect::<Vec<_>>())
        .collect();
    assert_eq!(hooks, ["ActiveSupport.run_load_hooks(:ts_gem, TsGem::Base)"], "{}", dir.display());
}

fn walk(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push(std::fs::read_to_string(&path).unwrap());
        }
    }
    out
}
