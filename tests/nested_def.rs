//! A nested `def` (in a block or a method body) has no IR node. It used to ingest as a bare nil,
//! so `Class.new(Parent) { private def m = ... }` emitted as
//! `{ private nil }` with no diagnostic: a class missing the method it
//! was made for. The site is now a refusal, an error in app code and a
//! raise in the emit.

use roundhouse::App;
use roundhouse::analyze::{Analyzer, Severity, diagnose};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

#[test]
fn a_def_inside_a_block_is_an_error_and_emits_a_raise() {
    let source = r#"
class Util
  def build
    Class.new(StandardError) { private def detail = "x" }
  end
end
"#;
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    app.library_classes.extend(classes);
    Analyzer::new(&app).analyze(&mut app);

    let errors: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error && d.message.contains("nested def"))
        .collect();
    assert_eq!(errors.len(), 1, "expected one nested-def error: {errors:?}");

    let out = emit_library(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!out.contains("private nil"), "the def must not vanish silently:\n{out}");
    assert!(out.contains("raise \"roundhouse: nested def"), "the site should raise:\n{out}");
}
