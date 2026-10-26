use super::emit_and_run;

const SOURCE: &str = r#"class RoundingProbe
  def self.rate
    1.25.round(1)
  end

  def self.whole
    1.5.round
  end

  def self.zero_digits
    1.25.ceil(0)
  end

  def self.tens
    15.0.floor(-1)
  end

  def self.even
    2.5.round(half: :even)
  end

  def self.even_tenths
    1.25.round(1, half: :even)
  end

  def self.cut
    1.25.truncate(1)
  end

  def self.scaled(digits)
    1.25.round(digits)
  end
end
"#;

const ASSERTIONS: &str = r#"
got = [RoundingProbe.rate, RoundingProbe.whole, RoundingProbe.zero_digits, RoundingProbe.tens,
       RoundingProbe.even, RoundingProbe.even_tenths, RoundingProbe.cut,
       RoundingProbe.scaled(1), RoundingProbe.scaled(0)]
want = [1.3, 2, 2, 10, 2, 1.2, 1.2, 1.3, 1]
raise "values differ: #{got.inspect}" unless got == want
raise "classes differ: #{got.map(&:class).inspect}" unless got.map(&:class) == [Float, Integer, Integer, Integer, Integer, Float, Float, Float, Integer]
puts "Float rounding digits passed"
"#;

#[test]
fn float_rounding_class_follows_the_digits_argument() {
    let run = emit_and_run::real_blog()
        .write("app/models/rounding_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("Float rounding digits passed"));
}

/// The sidecar signature is what Spinel's `--rbs` holds the body to.
#[test]
fn float_rounding_sidecar_returns_follow_the_digits_argument() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/models/rounding_probe.rb", SOURCE)
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let sig = ["sig/app/models/rounding_probe.rbs", "app/models/rounding_probe.rbs"]
        .iter()
        .find_map(|p| std::fs::read_to_string(emitted.join(p)).ok())
        .expect("rounding_probe.rbs");
    let ret = |name: &str| {
        sig.lines()
            .find(|l| l.trim_start().starts_with(&format!("def self.{name}:")))
            .and_then(|l| l.rsplit("-> ").next())
            .map(str::to_owned)
            .unwrap_or_else(|| panic!("no {name} in:\n{sig}"))
    };
    for name in ["rate", "even_tenths", "cut"] {
        assert_eq!(ret(name), "Float", "{name}:\n{sig}");
    }
    for name in ["whole", "zero_digits", "tens", "even"] {
        assert_eq!(ret(name), "Integer", "{name}:\n{sig}");
    }
    let scaled = ret("scaled");
    assert!(scaled.contains("Integer") && scaled.contains("Float"), "scaled:\n{sig}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn float_rounding_digits_run_on_spinel() {
    emit_and_run::real_blog()
        .write("app/models/rounding_probe.rb", SOURCE)
        .run_spinel_with_rbs(ASSERTIONS)
        .assert_passes();
}
