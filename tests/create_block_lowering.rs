//! Block-form create inlining (`lower::apply_create_block_inline`).
//!
//! Shape tests over the factory rewrite: the bang form inlines to
//! new/body/raise-unless-save/value, the plain form saves without the
//! raise, `self.create!` drops the explicit receiver on `new`
//! (spinel#2157), and a non-single-param block stays put with a
//! `lower_residue` warning.

use roundhouse::analyze::{Analyzer, Diagnostic};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_create_block_inline;
use roundhouse::App;

fn lower_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    let diags = apply_create_block_inline(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn bang_form_inlines_with_raise_unless_save() {
    let (out, diags) = lower_and_emit(
        r#"
class Keystore
  def self.put(key, value)
    Keystore.create! do |kv|
      kv.key = key
      kv.value = value
    end
  end
end
"#,
    );
    assert!(!out.contains("create!"), "factory call should be inlined:\n{out}");
    assert!(out.contains("kv = Keystore.new"), "expected the new-binding:\n{out}");
    assert!(out.contains("kv.key = key"), "block body must survive inline:\n{out}");
    assert!(
        out.contains("ActiveRecord::RecordInvalid"),
        "bang form needs the raise arm:\n{out}"
    );
    assert!(out.contains("kv.save"), "{out}");
    assert!(diags.is_empty(), "matching site should not produce residue: {diags:?}");
}

#[test]
fn plain_form_saves_without_raise() {
    let (out, diags) = lower_and_emit(
        r#"
class Tagger
  def log(reason)
    Moderation.create do |m|
      m.reason = reason
    end
  end
end
"#,
    );
    assert!(!out.contains("create do"), "{out}");
    assert!(out.contains("m = Moderation.new"), "{out}");
    assert!(
        !out.contains("RecordInvalid"),
        "plain form must not raise:\n{out}"
    );
    assert!(out.contains("m.save"), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn self_receiver_becomes_bare_new() {
    let (out, _) = lower_and_emit(
        r#"
class Keystore
  def self.put(key)
    self.create! do |kv|
      kv.key = key
    end
  end
end
"#,
    );
    assert!(!out.contains("self.new"), "spinel#2157: emit bare new:\n{out}");
    assert!(out.contains("kv = new"), "{out}");
}

#[test]
fn non_single_param_block_keeps_shape_with_residue() {
    let (out, diags) = lower_and_emit(
        r#"
class Tagger
  def log
    Moderation.create do
      puts "side effect"
    end
  end
end
"#,
    );
    assert!(out.contains("create"), "unmatched block form must stay put:\n{out}");
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
    assert_eq!(diags[0].code(), "lower_residue");
}
