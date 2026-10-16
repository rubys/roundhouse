//! Modules and classes declared inside `class << self`.
//!
//! A namespace keeps helper modules inside `class << self`, and its
//! class methods return them by bare name. Ingest did not look into
//! that body, so both modules were dropped and each read emitted a
//! `roundhouse: constant not supported` raise. They are hoisted into the
//! enclosing scope, as the constants such a body writes already are.
//!
//! Natively the helpers live on the singleton class, so `Outer::Alpha`
//! from outside raises NameError; the hoisted declaration is reachable
//! by that name. That widening is deliberate and not asserted either
//! way. A name the enclosing body also declares is two constants
//! natively, and is refused instead of merged.
//!
//! Native Ruby on the same source is the oracle for what the program
//! prints through the singleton's own methods.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

const NAMESPACE: &str = r#"module Outer
  module Registry
    class << self
      def pick(key)
        case key
        when "a" then Alpha
        else Beta
        end
      end

      module Alpha
        class << self
          def dump(x) = "alpha:" + x.to_s
        end
      end

      class Beta
        def self.dump(x) = "beta:" + x.to_s
      end
    end
  end
end
"#;

const REPORT: &str = "puts Outer::Registry.pick(\"a\").dump(1)\nputs Outer::Registry.pick(\"b\").dump(2)\n";
const EXPECTED: &str = "alpha:1\nbeta:2\n";

#[test]
fn native_ruby_reads_singleton_body_modules_through_the_singleton_methods() {
    let dir = std::env::temp_dir().join(format!("roundhouse-singleton-mods-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("registry.rb"), NAMESPACE).unwrap();
    let output = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("require {:?}\n{REPORT}", dir.join("registry.rb")))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}

#[test]
fn a_module_in_a_singleton_body_runs_through_the_singleton_methods() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/outer/registry.rb", NAMESPACE)
        .run_ruby(REPORT);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
    let registry = std::fs::read_to_string(run.emitted.join("app/models/outer/registry.rb")).expect("registry.rb");
    assert!(!registry.contains("roundhouse:"), "{registry}");
}

/// `Alpha` is both the namespace's own module and the singleton body's:
/// natively `pick` returns the singleton's, which hoisting would merge
/// with the namespace's.
#[test]
fn a_name_declared_in_the_body_and_its_singleton_stays_refused() {
    let colliding = r#"module Outer
  module Registry
    module Alpha
      def self.dump(x) = "outer:" + x.to_s
    end

    class << self
      def pick = Alpha

      module Alpha
        def self.dump(x) = "singleton:" + x.to_s
      end
    end
  end
end
"#;
    let tree = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/outer/registry.rb", colliding),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let err = match ingest_app_from_tree(tree) {
        Ok(mut app) => {
            roundhouse::session::analyze_and_lower(&mut app);
            diagnose(&app).into_iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n")
        }
        Err(err) => err.to_string(),
    };
    assert!(err.contains("Alpha") && err.contains("class << self"), "{err}");
}
