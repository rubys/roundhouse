//! Native Ruby emission keeps byte values, evaluation count, and block effects.
use super::emit_and_run;

/// Overlay only the primitive probe; the standard fixture supplies a complete
/// emitted boot so this regression does not depend on handcrafted Rails setup.
fn byte_probe() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(
        "app/lib/byte_probe.rb",
        r#"class ByteProbe
  def initialize
    @calls = 0
  end
  def receiver
    @calls += 1
    "A\0é東🎉"
  end
  def values
    receiver.bytes
  end
  def nil_values
    receiver.bytes(&nil)
  end
  def calls
    @calls
  end
  def block_value
    "é".bytes { |byte| @calls += byte }
  end
end
"#,
    )
}

const ASSERTIONS: &str = r#"
require_relative "app/models/byte_probe"
probe = ByteProbe.new
raise "wrong bytes" unless probe.values == [65, 0, 195, 169, 230, 157, 177, 240, 159, 142, 137]
raise "receiver evaluated more than once" unless probe.calls == 1
raise "wrong nil-block bytes" unless probe.nil_values == [65, 0, 195, 169, 230, 157, 177, 240, 159, 142, 137]
raise "nil-block receiver evaluated more than once" unless probe.calls == 2
raise "wrong block return" unless probe.block_value == "é"
raise "block disappeared" unless probe.calls == 366
puts "String#bytes emitted contract passed"
"#;

/// Both runtimes execute byte methods natively, but Spinel also consumes the
/// generated signature, so a passing CRuby run alone cannot certify this fix.
fn assert_byte_signatures(path: &std::path::Path) {
    let rbs = std::fs::read_to_string(path).unwrap();
    assert!(rbs.contains("def values: () -> Array[Integer]"), "{rbs}");
    assert!(
        rbs.contains("def nil_values: () -> Array[Integer]"),
        "{rbs}"
    );
    assert!(rbs.contains("def block_value: () -> String"), "{rbs}");
}

/// Integer sums distinguish executed block effects from both a dropped block
/// and string-valued bytes, while the call counter exposes receiver duplication.
#[test]
fn string_bytes_preserves_values_and_block_effects() {
    let run = byte_probe().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert_byte_signatures(&run.emitted.join("sig/app/models/byte_probe.rbs"));
}

/// Compile the same source and consume its inferred byte-array RBS on Spinel.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn string_bytes_preserves_values_and_block_effects_on_spinel() {
    use std::process::Command;
    let (emitted, errors) = byte_probe().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    assert_byte_signatures(&emitted.join("app/models/byte_probe.rbs"));
    std::fs::write(
        emitted.join("contract.rb"),
        format!("require_relative \"boot\"\n{ASSERTIONS}"),
    )
    .unwrap();
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let extractor = std::path::Path::new(&compiler).with_file_name("spinel_rbs_extract");
    let seeds = Command::new(extractor)
        .arg(".")
        .current_dir(&*emitted)
        .output()
        .expect("extract emitted RBS seeds");
    assert!(
        seeds.status.success(),
        "{}",
        String::from_utf8_lossy(&seeds.stderr)
    );
    let seed_text = String::from_utf8_lossy(&seeds.stdout);
    let mut in_probe = false;
    let mut probe_seeds = Vec::new();
    for line in seed_text.lines() {
        if line.starts_with("class ") {
            in_probe = line == "class ByteProbe";
        } else if in_probe {
            probe_seeds.push(line);
        }
    }
    assert!(!probe_seeds.is_empty(), "{seed_text}");
    for method in ["values", "nil_values"] {
        assert!(
            probe_seeds
                .iter()
                .any(|line| line.starts_with(&format!("meth {method} int_array "))),
            "{seed_text}"
        );
    }
    std::fs::write(emitted.join("rbs-seeds.txt"), &seeds.stdout).unwrap();
    let compiled = Command::new(&compiler)
        .args(["--rbs", ".", "contract.rb", "-o", "contract"])
        .current_dir(&*emitted)
        .output()
        .expect("compile with generated RBS");
    std::fs::write(emitted.join("compile.stdout"), &compiled.stdout).unwrap();
    std::fs::write(emitted.join("compile.stderr"), &compiled.stderr).unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(!String::from_utf8_lossy(&compiled.stderr).contains("type seeds are unavailable"));
    let executed = Command::new(emitted.join("contract"))
        .current_dir(&*emitted)
        .output()
        .expect("run native byte contract");
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&executed.stdout).trim(),
        "String#bytes emitted contract passed"
    );
}
