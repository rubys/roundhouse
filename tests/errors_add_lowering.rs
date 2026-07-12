//! `errors.add` grounding (`lower::apply_errors_add_lowering`).
//!
//! Shape tests over the accumulator rewrite: self-receiver `add` calls
//! ground to `errors << "Humanized msg"` (:base bare, dynamic message
//! interpolated, missing message defaulted), and non-self receivers
//! keep their dynamic call with a `lower_residue` warning.

use roundhouse::analyze::{Analyzer, Diagnostic};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_errors_add_lowering;
use roundhouse::App;

fn lower_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    let diags = apply_errors_add_lowering(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn field_and_literal_message_ground_humanized() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def validate_body
    errors.add(:short_id, "is taken")
  end
end
"#,
    );
    assert!(!out.contains("errors.add"), "site should be grounded:\n{out}");
    // Rails humanize strips a trailing `_id`: :short_id → "Short".
    assert!(
        out.contains(r#"errors << "Short is taken""#),
        "expected humanized accumulator push:\n{out}"
    );
    assert!(diags.is_empty(), "self-receiver site should not produce residue: {diags:?}");
}

#[test]
fn base_contributes_bare_message() {
    let (out, _) = lower_and_emit(
        r#"
class Draft
  def validate_body
    errors.add(:base, "record stale")
  end
end
"#,
    );
    assert!(
        out.contains(r#"errors << "record stale""#),
        ":base must not be humanized into the message:\n{out}"
    );
}

#[test]
fn missing_message_defaults_to_is_invalid() {
    let (out, _) = lower_and_emit(
        r#"
class Draft
  def validate_body
    errors.add(:title)
  end
end
"#,
    );
    assert!(
        out.contains(r#"errors << "Title is invalid""#),
        "expected the Rails default message:\n{out}"
    );
}

#[test]
fn dynamic_message_interpolates_after_field() {
    let (out, _) = lower_and_emit(
        r#"
class Draft
  def validate_body(why)
    errors.add(:title, why)
  end
end
"#,
    );
    assert!(!out.contains("errors.add"), "{out}");
    assert!(
        out.contains(r#"errors << "Title #{why}""#),
        "expected interpolated message:\n{out}"
    );
}

#[test]
fn non_self_receiver_keeps_dispatch_with_residue() {
    let (out, diags) = lower_and_emit(
        r#"
class Reviewer
  def flag(record)
    record.errors.add(:title, "not yours")
  end
end
"#,
    );
    assert!(
        out.contains("errors.add"),
        "outside-the-record add must keep its dynamic call:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
    assert_eq!(diags[0].code(), "lower_residue");
    assert!(
        diags[0].message.contains("non-self errors receiver"),
        "{:?}",
        diags[0].message
    );
}
