//! A class that defines its own `then` does not yield its receiver.
//!
//! Kernel#then yields `self`, so the analyzer binds the block parameter
//! to the receiver's type. A promise's `then` yields the fulfilled value
//! instead (`deferred.then { |owned| owned.include?(tag) }`); binding it
//! to the promise made `include?` read as Module#include? on a promise
//! instance and refused it. Native Ruby on the same source is the oracle.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const TAGS: &str = r#"module Tags
  class Deferred
    def initialize(value) = @value = value
    def then(on_fulfill = nil, &block) = Deferred.new((on_fulfill || block).call(@value))
    def value = @value
  end

  class Owner
    #: -> Deferred
    def load_tags = Deferred.new(["vip", "gold"])

    #: (Array[String]) -> Deferred
    def any_tag(tags) = load_tags.then { |owned| tags.any? { owned.include?(it) } }

    #: -> String
    def label = "owner".then { |s| s.upcase }
  end

  def self.report
    owner = Owner.new
    [owner.any_tag(["gold"]).value, owner.any_tag(["new"]).value, owner.label]
  end
end
"#;

const REPORT: &str = "p Tags.report\n";

#[test]
fn an_own_then_binds_its_block_parameter_as_native_ruby_does() {
    let dir = std::env::temp_dir().join(format!("roundhouse-own-then-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("tags.rb");
    std::fs::write(&file, TAGS).unwrap();
    let native = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("require {:?}\n{REPORT}", file.display().to_string()))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(expected, "[true, false, \"OWNER\"]\n", "native control");

    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("lib/tags.rb", TAGS)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!(
            "require \"./app/models/tags\"\nrequire \"./app/models/tags/deferred\"\nrequire \"./app/models/tags/owner\"\n{REPORT}"
        ))
        .current_dir(&emitted)
        .output()
        .expect("spawn ruby");
    assert!(output.status.success(), "emitted program failed in {}:\n{}", emitted.display(), String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}
