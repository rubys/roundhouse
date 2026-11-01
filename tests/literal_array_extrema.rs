#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const MODEL: &str = r#"class LiteralExtrema
  def self.time
    [Time.utc(2026, 1, 2), nil].compact.max
  end
  def self.string
    ["b", "a"].min
  end
  def self.integer
    [3, nil, 1].compact.max
  end
  def self.empty
    [].max
  end
  def self.possibly_empty(condition)
    candidate = condition ? Time.utc(2026, 1, 2) : nil
    [candidate].compact.max
  end
  def self.splat(values)
    [*values].max
  end
end
"#;

fn inferred(name: &str) -> Ty {
    inferred_from(MODEL, name)
}

fn inferred_from(source: &str, name: &str) -> Ty {
    let mut app = ingest_app_from_tree(
        [(
            std::path::PathBuf::from("app/models/literal_extrema.rb"),
            source.as_bytes().to_vec(),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    Analyzer::new(&app).analyze(&mut app);
    app.library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "LiteralExtrema")
        .unwrap()
        .methods
        .iter()
        .find(|method| method.name.as_str() == name)
        .unwrap()
        .body
        .ty
        .clone()
        .unwrap()
}

fn can_be_nil(ty: &Ty) -> bool {
    match ty {
        Ty::Nil | Ty::Untyped { .. } | Ty::Var { .. } => true,
        Ty::Union { variants } => variants.iter().any(can_be_nil),
        _ => false,
    }
}

#[test]
fn nonempty_literal_extrema_keep_the_selected_element_type() {
    assert_eq!(inferred("time"), Ty::Time);
    assert_eq!(inferred("string"), Ty::Str);
    assert_eq!(inferred("integer"), Ty::Int);
}

#[test]
fn empty_and_possibly_empty_extrema_stay_nullable() {
    for name in ["empty", "possibly_empty", "splat"] {
        assert!(can_be_nil(&inferred(name)), "{name}");
    }
}

#[test]
fn counted_literal_extrema_do_not_use_the_element_type() {
    let source = r#"class LiteralExtrema
  def self.counted
    [3, 1].max(2)
  end
end
"#;
    assert_ne!(inferred_from(source, "counted"), Ty::Int);
}

#[test]
fn direct_nullable_literal_extrema_preserve_existing_inference() {
    let source = r#"class LiteralExtrema
  def self.maximum
    [3, nil].max
  end
end
"#;
    assert_eq!(inferred_from(source, "maximum"), Ty::Int);
}

const NULLABLE_MODEL: &str = r#"class LiteralExtrema
  def self.single(condition)
    candidate = condition ? 3 : nil
    [candidate].max
  end

  def self.with_present(condition)
    candidate = condition ? 3 : nil
    [candidate, 3].max
  end
end
"#;

#[test]
fn a_single_nullable_element_keeps_a_nullable_extremum() {
    assert!(can_be_nil(&inferred_from(NULLABLE_MODEL, "single")));
}

#[test]
fn a_definitely_present_element_keeps_the_non_nil_extremum() {
    assert_eq!(inferred_from(NULLABLE_MODEL, "with_present"), Ty::Int);
}

const SCRIPT: &str = r#"
raise "time maximum changed" unless LiteralExtrema.time.iso8601 == "2026-01-02T00:00:00Z"
raise "string minimum changed" unless LiteralExtrema.string == "a"
raise "integer maximum changed" unless LiteralExtrema.integer == 3
raise "empty maximum changed" unless LiteralExtrema.empty.nil?
raise "nullable maximum changed" unless LiteralExtrema.possibly_empty(false).nil?
puts "literal array extrema passed"
"#;

#[test]
fn literal_array_extrema_run_in_emitted_ruby() {
    emit_and_run::empty_app()
        .write("app/models/literal_extrema.rb", MODEL)
        .run_ruby(SCRIPT)
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn literal_array_extrema_run_natively_with_emitted_rbs() {
    let run = emit_and_run::empty_app()
        .write("app/models/literal_extrema.rb", MODEL)
        .run_spinel_with_rbs(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, "literal array extrema passed\n");
}
