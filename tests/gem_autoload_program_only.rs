//! A gem `autoload` of a file that only requires its children loads that
//! file, eager loading or not.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;

const TRACKKIT: &str = r#"require "active_support"

module Trackkit
  extend ActiveSupport::Autoload

  autoload :Status
  autoload :Router
  autoload :Keys, "trackkit/keys"
end
"#;
const STATUS: &str = r#"module Trackkit
  module Status
    extend ActiveSupport::Autoload

    eager_autoload do
      autoload :Track
      autoload :Options
    end
  end
end
"#;
const OPTIONS: &str = "module Trackkit\n  module Status\n    class Options\n      def self.label = \"options\"\n    end\n  end\nend\n";
// Only requires its children, as relay's status/track.rb.
const TRACK: &str = "require_relative \"track/base\"\nrequire_relative \"track/none\"\n";
const BASE: &str =
    "module Trackkit\n  module Status\n    module Track\n      class Base\n        def self.label = \"base\"\n      end\n    end\n  end\nend\n";
const NONE: &str = "module Trackkit\n  module Status\n    module Track\n      class None < Base\n        def self.label = \"none\"\n      end\n    end\n  end\nend\n";
// Only nested declarations, as relay's command_table/router.rb.
const ROUTER: &str = r#"module Trackkit
  module Router
    module KeyspaceRouting
      def self.label = "keyspace"
    end

    class Queuing
      def self.label = "queuing"
    end
  end
end
"#;
const KEYS: &str = "require_relative \"keys/pair\"\n";
const PAIR: &str = "module Trackkit\n  module Keys\n    class Pair\n      def self.label = \"pair\"\n    end\n  end\nend\n";

const SCRIPT: &str = "require \"trackkit\" unless defined?(Trackkit)\n\
puts Trackkit::Status::Track::None.label\n\
puts Trackkit::Status::Track::None.superclass.label\n\
puts Trackkit::Status::Options.label\n\
puts Trackkit::Status::Track.class\n\
puts Trackkit::Router::Queuing.label\n\
puts Trackkit::Router::KeyspaceRouting.label\n\
puts Trackkit::Keys::Pair.label\n";

const LOCK: &str = "PATH\n  remote: vendor/gems/trackkit-1.0.0\n  specs:\n    trackkit (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  trackkit!\n";

const FILES: [(&str, &str); 9] = [
    ("trackkit.rb", TRACKKIT),
    ("trackkit/status.rb", STATUS),
    ("trackkit/status/options.rb", OPTIONS),
    ("trackkit/status/track.rb", TRACK),
    ("trackkit/status/track/base.rb", BASE),
    ("trackkit/status/track/none.rb", NONE),
    ("trackkit/router.rb", ROUTER),
    ("trackkit/keys.rb", KEYS),
    ("trackkit/keys/pair.rb", PAIR),
];

const EXPECTED: [&str; 7] = ["none", "base", "options", "Module", "queuing", "keyspace", "pair"];

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::to_string).collect()
}

/// The script on CRuby with the Rails oracle's Active Support, the gem
/// from its own lib/.
fn native(lib: &Path) -> Vec<String> {
    let oracle = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    // A file, not `-e`, and the load path set in it: the oracle
    // re-executes the program it runs, without the interpreter's flags.
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
#[ignore = "fork spec: autoload"]
fn a_gem_autoload_of_a_file_that_only_requires_its_children_loads_that_file() {
    program_only_autoload(false);
}

#[test]
#[ignore = "fork spec: autoload"]
fn a_gem_autoload_of_a_file_that_only_requires_its_children_loads_that_file_when_eager_loading() {
    program_only_autoload(true);
}

fn program_only_autoload(eager: bool) {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-autoload-program-only-{}-{eager}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("trackkit/lib");
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
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"trackkit\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write(
            "config/environments/development.rb",
            &format!("Rails.application.configure do\n  config.eager_load = {eager}\nend\n"),
        );
    for (path, text) in FILES {
        app = app.write(&format!("vendor/gems/trackkit-1.0.0/lib/{path}"), text);
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
    // The constants' autoloads, as the gem's own: the program for Track
    // and Keys (and the program announces itself, so a plain require of
    // it while the autoload is pending is no circular require), the
    // implicit module for Router with its nested constants' files.
    let models = std::fs::read_to_string(run.emitted.join("app/models.rb")).unwrap();
    let program = "models/roundhouse_files/vendor/gems/trackkit-1.0.0/lib/trackkit/status/track";
    assert!(models.contains(&format!("[:Track, {program:?}, ")), "{models}");
    assert!(models.contains(&format!("{program:?} => [[\"Trackkit::Status\", :Track]]")), "{models}");
    assert!(models.contains("[:Router, nil, true]"), "{models}");
    assert!(models.contains("\"Trackkit::Router\" => [[:KeyspaceRouting, \"models/trackkit/router/keyspace_routing\", true], [:Queuing, \"models/trackkit/router/queuing\", false]]"), "{models}");
    let track = std::fs::read_to_string(run.emitted.join(format!("app/{program}.rb"))).unwrap();
    assert!(track.contains(&format!("RoundhouseAutoload.entering_program({program:?})")), "{track}");
}
