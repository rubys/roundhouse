//! A load-time reference into a namespace a gem autoloads loads the
//! namespace's program first.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;

const TRACKOPTS: &str = r#"require "active_support"

module Trackopts
  extend ActiveSupport::Autoload

  autoload :Status
end
"#;
const STATUS: &str = r#"module Trackopts
  module Status
    extend ActiveSupport::Autoload

    eager_autoload do
      autoload :Track
      autoload :Options
    end
  end
end
"#;
// A load-time reference into Track, as relay's status/options.rb.
const OPTIONS: &str = "module Trackopts\n  module Status\n    module Options\n      DEFAULT_CONFIG = Trackopts::Status::Track::None::Config.new\n\n      def self.label = DEFAULT_CONFIG.label\n    end\n  end\nend\n";
// Only requires its children, as relay's status/track.rb.
const TRACK: &str = "require_relative \"track/base\"\nrequire_relative \"track/none\"\nrequire_relative \"track/status\"\n";
const BASE: &str = "module Trackopts\n  module Status\n    module Track\n      class Base\n        class Config\n          def label = \"base\"\n        end\n      end\n    end\n  end\nend\n";
const NONE: &str = "module Trackopts\n  module Status\n    module Track\n      class None < Base\n        class Config < Base::Config\n          def label = \"none\"\n        end\n      end\n    end\n  end\nend\n";
const TRACK_STATUS: &str = "module Trackopts\n  module Status\n    module Track\n      class Status < Base\n        class Config < Base::Config\n          def label = \"status\"\n        end\n      end\n    end\n  end\nend\n";

const SCRIPT: &str = "$VERBOSE = true\n\
def Warning.warn(message, **)\n  raise message if message.include?(\"circular require\")\n  super\nend\n\
require \"trackopts\" unless defined?(Trackopts)\n\
puts Trackopts::Status::Options.label\n\
puts Trackopts::Status::Track::Status::Config.new.label\n\
puts Trackopts::Status::Track::None.superclass.name\n";

const LOCK: &str = "PATH\n  remote: vendor/gems/trackopts-1.0.0\n  specs:\n    trackopts (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  trackopts!\n";

const FILES: [(&str, &str); 7] = [
    ("trackopts.rb", TRACKOPTS),
    ("trackopts/status.rb", STATUS),
    ("trackopts/status/options.rb", OPTIONS),
    ("trackopts/status/track.rb", TRACK),
    ("trackopts/status/track/base.rb", BASE),
    ("trackopts/status/track/none.rb", NONE),
    ("trackopts/status/track/status.rb", TRACK_STATUS),
];

const EXPECTED: [&str; 3] = ["none", "status", "Trackopts::Status::Track::Base"];

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::to_string).collect()
}

/// The script on CRuby with the Rails oracle's Active Support, the gem
/// from its own lib/.
fn native(lib: &Path) -> Vec<String> {
    let oracle = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let program = format!(
        "require {:?}\nRailsOracle.activate(\"activesupport\")\n$LOAD_PATH.unshift({:?})\n{SCRIPT}",
        oracle.display().to_string(),
        lib.display().to_string()
    );
    let script = lib.parent().unwrap().join("control.rb");
    std::fs::write(&script, program).unwrap();
    let out = emit_and_run::ruby().arg(&script).output().expect("CRuby control");
    assert!(out.status.success(), "native control failed\n{}", String::from_utf8_lossy(&out.stderr));
    lines(&out.stdout)
}

#[test]
fn a_load_time_reference_into_a_program_autoloaded_namespace_loads_the_program_first() {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-namespace-program-first-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("trackopts/lib");
    for (path, text) in FILES {
        let file = lib.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    assert_eq!(native(&lib), EXPECTED, "native control");
    let _ = std::fs::remove_dir_all(&root);

    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"trackopts\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write("config/environments/development.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n");
    for (path, text) in FILES {
        app = app.write(&format!("vendor/gems/trackopts-1.0.0/lib/{path}"), text);
    }
    let run = app.run_ruby(SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    assert_eq!(lines(run.stdout.as_bytes()), EXPECTED, "emitted boot diverges from CRuby\n{}", run.stderr);
}
