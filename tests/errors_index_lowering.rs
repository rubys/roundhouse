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
fn whole_messages_hash_reads_the_record_s_error_details() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  def problems
    errors.messages.compact_blank
  end

  def problems_on(field)
    errors.messages[field]
  end
end
"#,
    );
    assert!(out.contains("ActiveSupport.errors_messages(error_details).compact_blank"), "{out}");
    assert!(out.contains("ActiveSupport.errors_messages(error_details)[field]"), "a field that is not a literal reads the Hash:\n{out}");
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn membership_asks_the_attribute_s_messages() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  include ActiveModel::Validations

  def problems
    [errors.include?(:url), errors.key?("url"), Draft.new.errors.has_key?(:base)]
  end
end
"#,
    );
    assert!(out.contains(r#"error_messages_for("url").any?, error_messages_for("url").any?"#), "{out}");
    assert!(out.contains(r#"Draft.new.error_messages_for("base").any?"#), "{out}");
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn membership_on_a_dynamic_field_is_an_error() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  include ActiveModel::Validations

  def problem?(field)
    errors.include?(field)
  end
end
"#,
    );
    assert!(out.contains("errors.include?(field)"), "left alone:\n{out}");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].contains("errors.include?"), "{diags:?}");
}

#[test]
fn attribute_names_reads_the_record_s_error_details() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  include ActiveModel::Validations

  def problem_names
    errors.attribute_names
  end
end
"#,
    );
    assert!(out.contains("ActiveSupport.errors_attribute_names(error_details)"), "{out}");
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
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


#[test]
fn an_app_s_own_errors_array_is_left_alone() {
    let (out, diags) = lower_and_emit(
        r#"
class Probe
  def errors
    [:timeout]
  end

  def timed_out?
    errors.include?(:timeout)
  end
end
"#,
    );
    assert!(out.contains("errors.include?(:timeout)"), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn iterating_errors_iterates_the_error_details() {
    let (out, diags) = lower_and_emit(
        r#"
class Draft
  include ActiveModel::Validations

  def problems
    errors.map { |error| [error.attribute, error.full_message] }
  end

  def base_problem
    Draft.new.errors.find { |error| error.attribute == :base }
  end

  def first_attribute
    errors.first.attribute
  end
end
"#,
    );
    assert!(out.contains("error_details.map { |error| [error.attribute.to_sym, error.full_message] }"), "{out}");
    assert!(out.contains("Draft.new.error_details.find { |error| error.attribute.to_sym == :base }"), "{out}");
    assert!(out.contains("error_details.first.attribute.to_sym"), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn a_blockless_count_or_question_stays_on_the_messages() {
    let (out, _) = lower_and_emit(
        r#"
class Draft
  include ActiveModel::Validations

  def summary
    [errors.count, errors.any?, errors.count { |error| error.attribute == :base }]
  end
end
"#,
    );
    assert!(out.contains("[errors.count, errors.any?, error_details.count { |error| error.attribute.to_sym == :base }]"), "{out}");
}
