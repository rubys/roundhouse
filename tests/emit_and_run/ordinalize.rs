use super::emit_and_run;

const SOURCE: &str = r#"class OrdinalizeProbe
  def self.boundaries
    [1.ordinalize, 2.ordinalize, 3.ordinalize, 4.ordinalize, 11.ordinalize, 12.ordinalize, 13.ordinalize, 21.ordinalize, 22.ordinalize, 23.ordinalize, 101.ordinalize, 111.ordinalize]
  end

  def self.negative
    -23.ordinalize
  end
end
"#;

const ASSERTIONS: &str = r#"
raise "ordinal boundaries differ" unless OrdinalizeProbe.boundaries == ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd", "101st", "111th"]
raise "negative ordinal differs" unless OrdinalizeProbe.negative == "-23rd"
puts "Integer ordinalize passed"
"#;

#[test]
fn integer_ordinalize_runs_in_emitted_ruby() {
    let run = emit_and_run::real_blog()
        .write("app/models/ordinalize_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("Integer ordinalize passed"));

    let emitted = std::fs::read_to_string(run.emitted.join("app/models/ordinalize_probe.rb"))
        .expect("emitted ordinalize caller");
    assert!(
        emitted.contains("ActiveSupport::Inflector.ordinalize(1)"),
        "{emitted}"
    );
}
