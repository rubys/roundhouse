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

/// No static type to ground on, so the value goes to the runtime
/// predicate rather than staying a dynamic `Object#present?` send that
/// only CRuby's core_ext reopen could serve.
#[test]
fn untyped_receiver_routes_to_the_runtime_predicate() {
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
    assert!(
        out.contains("ActiveSupport.present?(thing)"),
        "untyped receiver should become a runtime predicate call:\n{out}"
    );
    assert!(
        !out.contains("thing.present?"),
        "the dynamic send must not survive:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

/// The runtime predicate reads its argument once, so a receiver with
/// effects grounds here where the type-directed forms must refuse it.
#[test]
fn impure_untyped_receiver_grounds_because_the_value_is_an_argument() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check(rec)
    return "yes" if rec.save!.present?
    "no"
  end
end
"#,
    );
    assert!(
        out.contains("ActiveSupport.present?(rec.save!)"),
        "impure receiver should be evaluated once as an argument:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

/// An app class that defines its own predicate keeps normal dispatch
/// even through a nilable union — the runtime helper knows nothing
/// about the app's definition and must not swallow it.
#[test]
fn nilable_own_predicate_class_still_refuses() {
    let (_out, diags) = lower_and_emit(
        r#"
class Bag
  def present?
    true
  end
end

class Util
  def check(flag)
    b = flag ? Bag.new : nil
    return "yes" if b.present?
    "no"
  end
end
"#,
    );
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
