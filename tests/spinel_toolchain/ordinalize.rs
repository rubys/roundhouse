use super::emit_and_run;
use roundhouse::project::BuildTarget;
use std::process::Command;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn integer_ordinalize_runs_natively() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/models/ordinalize_probe.rb",
            r#"class OrdinalizeProbe
  def self.boundaries
    [1.ordinalize, 2.ordinalize, 3.ordinalize, 4.ordinalize, 11.ordinalize, 12.ordinalize, 13.ordinalize, 21.ordinalize, 22.ordinalize, 23.ordinalize, 101.ordinalize, 111.ordinalize]
  end

  def self.negative
    -23.ordinalize
  end
end
"#,
        )
        .emit(BuildTarget::Spinel);
    assert!(
        errors.is_empty(),
        "analysis/emission errors: {}",
        errors.join("\n")
    );

    // Compile only the shared inflections runtime and emitted caller. The
    // full real-blog boot pulls in unrelated CGI/cache APIs outside this
    // focused native contract.
    let caller = std::fs::read_to_string(emitted.join("app/models/ordinalize_probe.rb"))
        .expect("emitted ordinalize caller");
    assert!(
        caller.contains("ActiveSupport::Inflector.ordinalize(1)"),
        "{caller}"
    );
    std::fs::write(
        emitted.join("contract.rb"),
        r#"require_relative "runtime/active_support_inflections"
require_relative "app/models/ordinalize_probe"
raise "ordinal boundaries differ" unless OrdinalizeProbe.boundaries == ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd", "101st", "111th"]
raise "negative ordinal differs" unless OrdinalizeProbe.negative == "-23rd"
puts "Integer ordinalize native passed"
"#,
    )
    .expect("write native caller");

    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let compiled = Command::new(&compiler)
        .args(["contract.rb", "-o", "contract"])
        .current_dir(&*emitted)
        .output()
        .expect("spawn Spinel compiler");
    assert!(
        compiled.status.success(),
        "Spinel compilation failed:\n{}\n{}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );

    let output = Command::new(emitted.join("contract"))
        .current_dir(&*emitted)
        .output()
        .expect("run native ordinalize contract");
    assert!(
        output.status.success(),
        "native ordinalize contract failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Integer ordinalize native passed"));
}
