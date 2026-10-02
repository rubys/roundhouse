//! `module_function` in a library-class body.
//!
//! Two spellings reach ingest: the bare marker (flips every subsequent
//! direct `def` to a module method) and the argument form
//! (`module_function :a, :b`, which names already-defined methods).
//! Only the bare form was handled; the argument form fell through and
//! its methods stayed instance-only, so the module spelling every
//! caller uses — `Mod.thing(...)` — resolved to nothing.

use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::App;

fn emit(source: &str) -> String {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn argument_form_promotes_the_named_method() {
    let out = emit(
        r#"
module Blocklist
  def on_blocklist?(email)
    email.end_with?("@example.com")
  end

  module_function :on_blocklist?
end
"#,
    );
    assert!(
        out.contains("def self.on_blocklist?"),
        "named method should become a module method:\n{out}"
    );
}

#[test]
fn argument_form_leaves_unnamed_methods_alone() {
    let out = emit(
        r#"
module Blocklist
  def named(email)
    email
  end

  def unnamed(email)
    email
  end

  module_function :named
end
"#,
    );
    assert!(out.contains("def self.named"), "named should promote:\n{out}");
    assert!(
        out.contains("def unnamed"),
        "unnamed should stay an instance method:\n{out}"
    );
    assert!(
        !out.contains("def self.unnamed"),
        "unnamed must not be promoted:\n{out}"
    );
}

#[test]
fn sibling_bare_calls_retarget_to_the_module_spelling() {
    // Ruby keeps a private instance copy, so a bare sibling call still
    // works there. We emit one method per name, so the sibling call has
    // to move to the module spelling or it resolves to nothing.
    let out = emit(
        r#"
module Blocklist
  def validate(email)
    errors << "blocked" if on_blocklist?(email)
  end

  def on_blocklist?(email)
    email.end_with?("@example.com")
  end

  module_function :on_blocklist?
end
"#,
    );
    assert!(
        out.contains("Blocklist.on_blocklist?(email)"),
        "sibling bare call should retarget to the module spelling:\n{out}"
    );
}

#[test]
fn an_explicit_receiver_is_not_retargeted() {
    // Only receiver-less sends are ours to move; `other.on_blocklist?`
    // is a call on someone else's object that happens to share a name.
    let out = emit(
        r#"
module Blocklist
  def check(other, email)
    other.on_blocklist?(email)
  end

  def on_blocklist?(email)
    email
  end

  module_function :on_blocklist?
end
"#,
    );
    assert!(
        out.contains("other.on_blocklist?(email)"),
        "explicit receiver should survive untouched:\n{out}"
    );
}

#[test]
fn bare_marker_still_flips_subsequent_defs() {
    // Regression guard on the pre-existing behaviour.
    let out = emit(
        r#"
module Util
  def before(x)
    x
  end

  module_function

  def after(x)
    x
  end
end
"#,
    );
    assert!(out.contains("def before"), "pre-marker def stays instance:\n{out}");
    assert!(!out.contains("def self.before"), "pre-marker def must not flip:\n{out}");
    assert!(out.contains("def self.after"), "post-marker def flips:\n{out}");
}

#[test]
fn module_function_copies_are_public_even_when_the_instance_definition_is_private() {
    use roundhouse::dialect::{MethodReceiver, MethodVisibility};
    for source in [
        "module Util; private; def helper; 11; end; module_function :helper; end",
        "module Util; private; module_function; def helper; 12; end; end",
        "module Util; module_function; private def helper; 13; end; def later; 14; end; end",
    ] {
        let classes = ingest_library_classes(source.as_bytes(), "util.rb").unwrap();
        for method in &classes[0].methods {
            assert_eq!(method.receiver, MethodReceiver::Class, "{source}");
            assert_eq!(method.visibility, MethodVisibility::Public, "{source}");
        }
        let out = emit(source);
        assert!(!out.contains("private_class_method"), "{out}");
    }
}

#[test]
fn only_bare_instance_visibility_markers_end_module_function_mode() {
    use roundhouse::dialect::{MethodReceiver, MethodVisibility};
    for (marker, visibility) in [
        ("public", MethodVisibility::Public),
        ("protected", MethodVisibility::Protected),
        ("private", MethodVisibility::Private),
    ] {
        let source = format!(
            "module Util; module_function; def first; 21; end; private :first; \
             def second; 22; end; {marker}; def last; 23; end; end"
        );
        let classes = ingest_library_classes(source.as_bytes(), "util.rb").unwrap();
        let methods = &classes[0].methods;
        for method in &methods[..2] {
            assert_eq!(method.receiver, MethodReceiver::Class, "{source}");
            assert_eq!(method.visibility, MethodVisibility::Public, "{source}");
        }
        assert_eq!(methods[2].receiver, MethodReceiver::Instance, "{source}");
        assert_eq!(methods[2].visibility, visibility, "{source}");
    }
}
