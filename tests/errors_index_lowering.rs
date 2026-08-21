//! `errors[:field]` grounding (`lower::apply_errors_index_lowering`).
//!
//! Shape tests over the projection: the framework runtime's `errors` is
//! an `Array[String]` of FULL messages, so Rails' per-field read becomes
//! `ActiveSupport.errors_for(errors, "<Humanized> ")` — a prefix strip
//! against the text `errors.add` / `validates` baked. The receiver of
//! `errors` survives, `:base` declines (it carries no prefix), and an
//! `[]` on anything that is not an `errors` reader is left alone.

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
fn self_receiver_grounds_to_the_prefix_strip() {
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
        out.contains(r#"ActiveSupport.errors_for(errors, "Url ")"#),
        "expected the grounded projection:\n{out}"
    );
    assert!(diags.is_empty(), "unexpected residue: {diags:?}");
}

#[test]
fn multi_word_field_humanizes_like_the_bake() {
    // The prefix has to be spelled the way `errors.add` baked it, or
    // the strip matches nothing: `humanize` and this pass share one
    // implementation for exactly that reason.
    let (out, _) = lower_and_emit(
        r#"
class Draft
  def problems
    errors[:client_message_id]
  end
end
"#,
    );
    assert!(
        out.contains(r#""Client message ""#),
        "expected the humanized field plus its trailing space:\n{out}"
    );
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
        out.contains("record.errors"),
        "expected the foreign receiver preserved:\n{out}"
    );
}

#[test]
fn base_declines_with_residue() {
    // `:base` messages are baked with NO humanized prefix, so there is
    // no text to match; the site stays dynamic and is ledgered.
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
        out.contains("errors[:base]"),
        "`:base` should be left alone:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
    assert!(diags[0].contains("errors[:field]"), "{diags:?}");
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
