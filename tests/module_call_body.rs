//! A gem module whose body is only calls is declared and runs them, and a
//! constant whose file raised while loading autoloads again.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;

const SCHEMA: &str = r#"module Comp
  class Schema
    module BooleanType
      freeze
    end
    module SelfFrozen
      self.freeze
    end
    class Klass
      def self.label = "klass"
      freeze
    end
    def self.type_for(t)
      case t
      when BooleanType.singleton_class then :bool
      when SelfFrozen.singleton_class then :self_frozen
      else :other
      end
    end
  end
end
"#;

const MODELS_SCRIPT: &str = "puts Comp::Schema.type_for(Comp::Schema::BooleanType)\n\
puts Comp::Schema.type_for(Comp::Schema::SelfFrozen)\n\
puts Comp::Schema::BooleanType.frozen?\n\
puts Comp::Schema::SelfFrozen.frozen?\n\
puts Comp::Schema::Klass.label\n\
puts Comp::Schema::Klass.frozen?\n";

#[test]
#[ignore = "fork spec: autoload"]
fn a_module_whose_body_is_only_calls_is_declared_and_runs_them() {
    let run = emit_and_run::empty_app().write("app/models/comp/schema.rb", SCHEMA).run_ruby(MODELS_SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    // CRuby 4.0 on SCHEMA, as written.
    assert_eq!(run.stdout.lines().collect::<Vec<_>>(), ["bool", "self_frozen", "true", "true", "klass", "true"], "{}", run.stderr);
}

const BROKEN: &str = "module Broken\n  autoload :Track, \"broken/track\"\n\n  def self.label = \"broken\"\nend\n";
// A program: it only requires its child, whose body raises.
const TRACK: &str = "require_relative \"track/base\"\n";
const BASE: &str = "module Broken\n  module Track\n    class Base\n      LIMIT = Integer(\"x\")\n    end\n  end\nend\n";
const LOCK: &str = "PATH\n  remote: vendor/gems/broken-1.0.0\n  specs:\n    broken (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  broken!\n";
const FILES: [(&str, &str); 3] = [("broken.rb", BROKEN), ("broken/track.rb", TRACK), ("broken/track/base.rb", BASE)];

const AUTOLOAD_SCRIPT: &str = "require \"broken\" unless defined?(Broken)\n\
2.times do\n\
  Broken::Track\n\
  puts \"loaded\"\n\
rescue StandardError => e\n\
  puts \"#{e.class}: #{e.message}\"\n\
end\n";

const RAISED: &str = "ArgumentError: invalid value for Integer(): \"x\"";

#[test]
#[ignore = "fork spec: autoload"]
fn a_constant_whose_program_raised_while_loading_autoloads_it_again() {
    let root = std::env::temp_dir().join(format!("roundhouse-module-call-body-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("broken/lib");
    for (path, text) in FILES {
        let file = lib.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let control = root.join("control.rb");
    std::fs::write(&control, format!("$LOAD_PATH.unshift({:?})\n{AUTOLOAD_SCRIPT}", lib.display().to_string())).unwrap();
    let out = emit_and_run::ruby().arg(&control).output().expect("CRuby control");
    let native = String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect::<Vec<_>>();
    assert_eq!(native, [RAISED, RAISED], "native control\n{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&root);

    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"broken\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n");
    for (path, text) in FILES {
        app = app.write(&format!("vendor/gems/broken-1.0.0/lib/{path}"), text);
    }
    let run = app.run_ruby(AUTOLOAD_SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    let program = "models/roundhouse_files/vendor/gems/broken-1.0.0/lib/broken/track";
    let track = std::fs::read_to_string(run.emitted.join(format!("app/{program}.rb"))).unwrap();
    assert!(track.contains(&format!("RoundhouseAutoload.entering_program({program:?})")), "{track}");
    assert!(Path::new(&run.emitted.join("app/models.rb")).exists());
    assert_eq!(run.stdout.lines().collect::<Vec<_>>(), [RAISED, RAISED], "emitted tree diverges from CRuby\n{}", run.stderr);
}
