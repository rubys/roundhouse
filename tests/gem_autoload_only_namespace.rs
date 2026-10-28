//! A gem `autoload` that names only a namespace loads it when it is read.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const PROFKIT: &str = r#"module Profkit
  LABEL = "profkit"

  class Error < StandardError
  end

  module Storage
    autoload :FileStorage, "profkit/storage/file_storage"
  end

  module Viewer
    autoload :Speedscope, "profkit/viewer/speedscope"
  end

  autoload(:Backend, "profkit/backend")

  def self.backend = Backend::Stack
  def self.viewer = Viewer::Speedscope
end
"#;
const BACKEND: &str = "module Profkit\n  module Backend\n    autoload :Stack, \"profkit/backend/stack\"\n  end\nend\n";
const STACK: &str = "module Profkit\n  module Backend\n    class Stack\n      def self.label = \"stack\"\n    end\n  end\nend\n";
const FILE_STORAGE: &str = "module Profkit\n  module Storage\n    class FileStorage\n      def self.label = \"file\"\n    end\n  end\nend\n";
const SPEEDSCOPE: &str = "module Profkit\n  module Viewer\n    class Speedscope\n      def self.label = \"speedscope\"\n    end\n  end\nend\n";

const SCRIPT: &str = "require \"profkit\" unless defined?(Profkit)\nputs Profkit.backend.label\nputs Profkit.viewer.label\nputs Profkit::Storage::FileStorage.label\nputs Profkit::Backend.class, Profkit::Storage.name\n";

const LOCK: &str = "PATH\n  remote: vendor/gems/profkit-1.0.0\n  specs:\n    profkit (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  profkit!\n";

const FILES: [(&str, &str); 5] = [
    ("profkit.rb", PROFKIT),
    ("profkit/backend.rb", BACKEND),
    ("profkit/backend/stack.rb", STACK),
    ("profkit/storage/file_storage.rb", FILE_STORAGE),
    ("profkit/viewer/speedscope.rb", SPEEDSCOPE),
];

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::to_string).collect()
}

#[test]
fn a_gem_namespace_holding_only_autoloads_is_defined() {
    autoload_only_namespace(false);
}

#[test]
fn a_gem_namespace_holding_only_autoloads_is_defined_when_eager_loading() {
    autoload_only_namespace(true);
}

fn autoload_only_namespace(eager: bool) {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-autoload-only-namespace-{}-{eager}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("profkit/lib");
    for (path, text) in FILES {
        let file = lib.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let native = emit_and_run::ruby().arg("-I").arg(&lib).args(["-e", SCRIPT]).output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = lines(&native.stdout);
    assert_eq!(expected, ["stack", "speedscope", "file", "Module", "Profkit::Storage"], "native control");
    let _ = std::fs::remove_dir_all(&root);

    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"profkit\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write(
            "config/environments/development.rb",
            &format!("Rails.application.configure do\n  config.eager_load = {eager}\nend\n"),
        );
    for (path, text) in FILES {
        app = app.write(&format!("vendor/gems/profkit-1.0.0/lib/{path}"), text);
    }
    let run = app.run_ruby(SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    assert_eq!(lines(run.stdout.as_bytes()), expected, "emitted boot diverges from CRuby");
}
