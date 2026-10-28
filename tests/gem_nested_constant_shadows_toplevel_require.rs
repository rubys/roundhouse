//! A gem's nested constant is the one its own file defines, not an app's
//! top-level constant of the same name.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const KITGEM: &str = r#"module Kitgem
  require "kitgem/format"

  Format.register_lazy(:json, "kitgem/json")

  def self.formats = Format.registry
end
"#;
const KITGEM_FORMAT: &str = "module Kitgem\n  class Format\n    def self.registry = (@registry ||= {})\n    def self.register_lazy(name, path) = registry[name] = path\n  end\nend\n";
const APP_FORMAT: &str = "module Format\n  EMAIL = /\\A[^@\\s]+@[^@\\s]+\\z/\n\n  def self.email?(value) = value =~ EMAIL\nend\n";

const SCRIPT: &str = "require \"kitgem\" unless defined?(Kitgem)\nputs Kitgem.formats.inspect\nputs Kitgem::Format.name, Format.name\nputs Format.email?(\"a@b.c\").inspect\n";
const NATIVE_PRELUDE: &str = "module Format\n  EMAIL = /\\A[^@\\s]+@[^@\\s]+\\z/\n  def self.email?(value) = value =~ EMAIL\nend\n";

const LOCK: &str = "PATH\n  remote: vendor/gems/kitgem-1.0.0\n  specs:\n    kitgem (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  kitgem!\n";

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::to_string).collect()
}

#[test]
#[ignore = "fork spec: load order"]
fn a_gem_nested_format_does_not_resolve_to_the_apps_toplevel_format() {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-nested-shadow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("kitgem/lib");
    std::fs::create_dir_all(lib.join("kitgem")).unwrap();
    std::fs::write(lib.join("kitgem.rb"), KITGEM).unwrap();
    std::fs::write(lib.join("kitgem/format.rb"), KITGEM_FORMAT).unwrap();
    let script = format!("{NATIVE_PRELUDE}{SCRIPT}");
    let native = emit_and_run::ruby().arg("-I").arg(&lib).args(["-e", &script]).output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = lines(&native.stdout);
    assert_eq!(expected, ["{json: \"kitgem/json\"}", "Kitgem::Format", "Format", "0"], "native control");
    let _ = std::fs::remove_dir_all(&root);

    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"kitgem\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write("app/models/format.rb", APP_FORMAT)
        .write("vendor/gems/kitgem-1.0.0/lib/kitgem.rb", KITGEM)
        .write("vendor/gems/kitgem-1.0.0/lib/kitgem/format.rb", KITGEM_FORMAT)
        .run_ruby(SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    assert_eq!(lines(run.stdout.as_bytes()), expected, "emitted boot diverges from CRuby");
}
