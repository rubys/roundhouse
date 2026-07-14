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
fn non_self_receiver_grounds_on_its_own_accumulator() {
    // `record.errors.add(...)` from outside the model (lobsters'
    // duplicate-comment guard) grounds the same way — the receiver is
    // preserved, so the append lands on record's accumulator.
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
        out.contains(r#"record.errors << "Title not yours""#),
        "outside-the-record add should ground with its receiver:\n{out}"
    );
    assert!(diags.is_empty(), "non-self site should ground without residue: {diags:?}");
}

#[test]
fn message_keyword_unwraps_to_the_message() {
    // Rails' options spelling: the sole `message:` entry carries the
    // message; any other option set stays residue.
    let (out, diags) = lower_and_emit(
        r#"
class Story
  def check
    errors.add(:moderation_reason, message: "is required")
  end
end
"#,
    );
    assert!(
        out.contains(r#"errors << "Moderation reason is required""#),
        "expected the message: value baked after the humanized field:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn non_message_options_keep_dispatch_with_residue() {
    let (out, diags) = lower_and_emit(
        r#"
class Story
  def check
    errors.add(:url, message: "is taken", strict: true)
  end
end
"#,
    );
    assert!(
        out.contains("errors.add"),
        "unrecognized option set must keep its dynamic call:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
    assert!(diags[0].message.contains("unrecognized arg shape"), "{:?}", diags[0].message);
}

#[test]
fn literal_concat_message_folds_before_bake() {
    // Lobsters wraps long messages as `"a " << "b"` — string-literal
    // concat folds into the baked literal (a runtime `<<` on a frozen
    // literal is a hazard the bake sidesteps).
    let (out, diags) = lower_and_emit(
        r#"
class Comment
  def check
    errors.add(:comment, "^You have already posted " << "here recently.")
  end
end
"#,
    );
    assert!(
        out.contains(r#"errors << "Comment ^You have already posted here recently.""#),
        "expected the concat folded into one literal:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}
