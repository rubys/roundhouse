//! Type-directed blank-predicate lowering (`lower::apply_blank_lowering`).
//!
//! Shape tests over the grounding table: string/collection receivers
//! ground through `empty?`, nilable receivers pick up the nil guard,
//! never-blank scalars fold, classes with their own predicate keep
//! normal dispatch, and ungroundable receivers survive verbatim with a
//! `blank_unlowered` residue diagnostic.

use roundhouse::analyze::{Analyzer, Diagnostic};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_blank_lowering;
use roundhouse::App;

fn lower_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    let diags = apply_blank_lowering(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn string_present_grounds_to_not_empty() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check
    s = "value"
    return "yes" if s.present?
    "no"
  end
end
"#,
    );
    assert!(!out.contains("present?"), "site should be grounded:\n{out}");
    assert!(out.contains("empty?"), "expected empty?-based form:\n{out}");
    assert!(diags.is_empty(), "typed receiver should not produce residue: {diags:?}");
}

#[test]
fn array_blank_grounds_to_empty() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check
    a = [1, 2]
    return "none" if a.blank?
    "some"
  end
end
"#,
    );
    assert!(!out.contains("blank?"), "site should be grounded:\n{out}");
    // The ruby-family emitter's nil-safety pass may wrap the receiver
    // (`(a || "").empty?`); either surface is the grounded form.
    assert!(out.contains(".empty?"), "expected empty?-based form:\n{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn nilable_string_present_gets_nil_guard() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check(flag)
    s = flag ? "value" : nil
    return "yes" if s.present?
    "no"
  end
end
"#,
    );
    assert!(!out.contains("present?"), "site should be grounded:\n{out}");
    assert!(out.contains("nil?"), "nilable receiver needs the nil guard:\n{out}");
    assert!(out.contains("empty?"), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn integer_present_folds_to_true() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check
    n = 3
    return "yes" if n.present?
    "no"
  end
end
"#,
    );
    assert!(!out.contains("present?"), "never-blank scalar should fold:\n{out}");
    assert!(!out.contains("empty?"), "no empty? for scalars:\n{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn string_presence_becomes_conditional() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def label(name)
    s = name.to_s
    s.presence || "anonymous"
  end
end
"#,
    );
    assert!(!out.contains("presence"), "presence should be grounded:\n{out}");
    assert!(out.contains("empty?"), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn untyped_receiver_survives_with_residue_diagnostic() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check(thing)
    return "yes" if thing.present?
    "no"
  end
end
"#,
    );
    assert!(out.contains("present?"), "ungroundable site must survive verbatim:\n{out}");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code(), "blank_unlowered");
}

#[test]
fn own_predicate_class_keeps_dispatch() {
    let (out, diags) = lower_and_emit(
        r#"
class Wrapper
  def blank?
    false
  end
end

class Util
  def check
    w = Wrapper.new
    return "yes" if w.blank?
    "no"
  end
end
"#,
    );
    assert!(
        out.contains("w.blank?"),
        "class with its own predicate keeps normal dispatch:\n{out}"
    );
    assert!(diags.is_empty(), "own-predicate dispatch is not residue: {diags:?}");
}

#[test]
fn indexed_read_receiver_is_reevaluable() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check(opts)
    h = { "a" => "x" }
    return "yes" if h["a"].present?
    "no"
  end
end
"#,
    );
    assert!(!out.contains("present?"), "hash-value read should ground:\n{out}");
    assert!(diags.is_empty(), "{diags:?}");
}
