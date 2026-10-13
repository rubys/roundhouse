//! `errors[:field]` grounding (`lower::apply_errors_index_lowering`).
//!
//! Shape tests over the projection: Rails' per-field read becomes
//! `error_messages_for`, a pick over the `error_details` `errors.add` /
//! `validates` keep for each message. The receiver of `errors` survives, and an `[]` on
//! anything that is not an `errors` reader is left alone.

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_errors_index_lowering;
use roundhouse::App;

fn lower_and_emit(source: &str) -> (String, Vec<String>) {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    let diags = apply_errors_index_lowering(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags.into_iter().map(|d| d.message).collect())
}

#[test]
fn self_receiver_grounds_to_the_attribute_pick() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def url_problems
    errors[:url]
  end
end
"#,
    );
    assert!(
        out.contains(r#"error_messages_for("url")"#),
        "expected the grounded projection:\n{out}"
    );
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn foreign_receiver_keeps_its_receiver() {
    // The receiver of `errors` is unconstrained and must survive —
    // dropping it would silently retarget the read to self.
    let (out, _) = lower_and_emit(
        r#"
class Draft
  def report(record)
    record.errors[:url]
  end
end
"#,
    );
    assert!(
        out.contains(r#"record.error_messages_for("url")"#),
        "expected the foreign receiver preserved:\n{out}"
    );
}

#[test]
fn base_picks_by_attribute() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def problems
    errors[:base]
  end
end
"#,
    );
    assert!(
        out.contains(r#"error_messages_for("base")"#),
        "expected the `:base` projection:\n{out}"
    );
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn dynamic_field_declines_with_residue() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def problems(field)
    errors[field]
  end
end
"#,
    );
    assert!(out.contains("errors[field]"), "dynamic field left alone:\n{out}");
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
}

#[test]
fn unrelated_index_receiver_is_untouched() {
    // Only an `errors` reader is our accumulator; an `[]` on anything
    // else keeps its call.
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def problems(report)
    report[:url]
  end
end
"#,
    );
    assert!(
        out.contains("report[:url]"),
        "non-errors receiver should be left alone:\n{out}"
    );
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn messages_key_asks_the_accumulator_for_the_field() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def url_bad?
    errors.messages.key?(:url)
  end
end
"#,
    );
    assert!(
        out.contains(r#"error_messages_for("url").any?"#),
        "expected `key?` as a non-empty field projection:\n{out}"
    );
    assert!(!out.contains("errors.messages"), "no `messages` should survive:\n{out}");
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn messages_index_reads_like_errors_index() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def report(record)
    record.errors.messages[:url]
  end
end
"#,
    );
    assert!(
        out.contains(r#"record.error_messages_for("url")"#),
        "expected the foreign receiver's projection:\n{out}"
    );
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn messages_emptiness_is_the_accumulator_emptiness() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def clean?
    errors.messages.blank?
  end

  def dirty?
    errors.messages.present?
  end
end
"#,
    );
    assert!(out.contains("errors || \"\").empty?"), "`blank?` → `empty?`:\n{out}");
    assert!(out.contains("errors.any?"), "`present?` → `any?`:\n{out}");
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn whole_messages_hash_declines_with_residue() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def problems
    errors.messages.compact_blank
  end
end
"#,
    );
    assert!(out.contains("errors.messages"), "left alone:\n{out}");
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
    assert!(diags[0].contains("errors.messages"), "{diags:?}");
}

#[test]
fn a_record_that_is_not_a_plain_read_is_read_once() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def problems
    Draft.latest.errors[:url]
  end
end
"#,
    );
    assert!(out.contains(r#"Draft.latest.error_messages_for("url")"#), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn to_hash_groups_the_record_s_error_details() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def problems
    errors.to_hash
  end

  def latest_problems
    Draft.latest.errors.to_hash
  end
end
"#,
    );
    assert!(out.contains("ActiveSupport.errors_to_hash(error_details)"), "expected the projection on self:\n{out}");
    assert!(
        out.contains("ActiveSupport.errors_to_hash(Draft.latest.error_details)"),
        "expected the projection to read the record once:\n{out}"
    );
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}
