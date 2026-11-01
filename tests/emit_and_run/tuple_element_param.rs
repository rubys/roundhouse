use super::emit_and_run;

/// A local that is either a one-class array or a two-class tuple
/// (`root ? [Paper.new(root)] : [Paper.new, Tape.new]`) is iterated, and
/// each element is passed to a method. The method's parameter is typed
/// from that call, so it must be the union of both classes. While a
/// block over a union receiver took its first variant's element alone,
/// the emitted RBS declared `describe: (PaperSource source)`; a larger
/// Spinel program built from such a signature failed with
/// `TypeError: wrong argument type ... (expected ...)` on the tuple's
/// second element.
fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/paper_source.rb",
            "class PaperSource
  def initialize(root)
    @root = root
  end

  def label = \"paper:#{@root}\"
end
",
        )
        .write("app/models/tape_source.rb", "class TapeSource\n  def label = \"tape\"\nend\n")
        .write(
            "app/models/catalog.rb",
            "class Catalog
  def self.default_sources = [PaperSource.new(\"shelf\"), TapeSource.new]

  def labels(root = nil)
    sources = root ? [PaperSource.new(root)] : self.class.default_sources
    sources.map { |source| describe(source) }
  end

  def describe(source) = source.label.upcase
end
",
        )
}

const ASSERTIONS: &str = r#"got = [Catalog.new.labels, Catalog.new.labels("box")]
raise got.inspect unless got == [["PAPER:SHELF", "TAPE"], ["PAPER:BOX"]]
puts "tuple element param passed"
"#;

#[test]
fn a_tuple_element_passed_to_a_method_types_its_param_as_the_union() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let rbs = std::fs::read_to_string(emitted.join("app/models/catalog.rbs")).expect("catalog.rbs emitted");
    assert!(rbs.contains("def describe: ((PaperSource | TapeSource) source)"), "{rbs}");
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("tuple element param passed"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_tuple_element_passed_to_a_method_types_its_param_as_the_union_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("tuple element param passed"));
}
