//! `to_fs(:delimited)` grounding (`lower::apply_number_to_fs_grounding`).

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_number_to_fs_grounding;
use roundhouse::App;

fn lower_and_emit(source: &str) -> String {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    apply_number_to_fs_grounding(&mut app);
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn integer_and_float_receivers_ground() {
    let out = lower_and_emit(
        r#"
class Report
  def total
    1234567.to_fs(:delimited)
  end

  def ratio
    1234.5.to_formatted_s(:delimited)
  end
end
"#,
    );
    assert!(out.contains("ActiveSupport.number_delimited(1234567)"), "{out}");
    assert!(out.contains("ActiveSupport.number_delimited(1234.5)"), "{out}");
}

#[test]
fn other_formats_and_untyped_receivers_stay() {
    let out = lower_and_emit(
        r#"
class Report
  def phone
    5551234.to_fs(:phone)
  end

  def total(count)
    count.to_fs(:delimited)
  end
end
"#,
    );
    assert!(out.contains("5551234.to_fs(:phone)"), "{out}");
    assert!(out.contains("count.to_fs(:delimited)"), "{out}");
    assert!(!out.contains("number_delimited"), "{out}");
}
