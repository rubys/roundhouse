//! Mailer class-side wrappers (`lower::apply_mailer_class_side`).
//!
//! Shape tests over the synthesis: a mailer instance method gains a
//! `def self.<name>` forwarding to `new.<name>(...)`, detection
//! follows the parent chain to ActionMailer::Base transitively, an
//! app-defined class-side method of the same name wins, non-mailer
//! classes are untouched, and keyword-param methods stay unwrapped
//! with a `lower_residue` warning. Native keyword declarations are
//! retained; the wrapper does not invent a positional ABI for them.

use roundhouse::App;

use roundhouse::analyze::{Analyzer, Diagnostic};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_mailer_class_side;

fn lower_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    let diags = apply_mailer_class_side(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn instance_method_gains_class_side_wrapper_through_parent_chain() {
    let (out, diags) = lower_and_emit(
        r#"
class ApplicationMailer < ActionMailer::Base
end

class BanNotification < ApplicationMailer
  def notify(user, banner, reason)
    mail(to: user, subject: reason)
  end
end
"#,
    );
    assert!(
        out.contains("def self.notify(user, banner, reason)"),
        "expected the class-side wrapper:\n{out}"
    );
    assert!(
        out.contains("new.notify(user, banner, reason)"),
        "wrapper forwards to the instance method:\n{out}"
    );
    assert!(diags.is_empty(), "positional methods produce no residue: {diags:?}");
}

#[test]
fn existing_class_side_method_wins() {
    let (out, _) = lower_and_emit(
        r#"
class Notifier < ActionMailer::Base
  def self.notify(user)
    "hand-rolled"
  end

  def notify(user)
    mail(to: user)
  end
end
"#,
    );
    assert!(out.contains("hand-rolled"), "{out}");
    assert!(
        !out.contains("new.notify"),
        "app-defined class-side method must not be shadowed:\n{out}"
    );
}

#[test]
fn non_mailer_class_is_untouched() {
    let (out, diags) = lower_and_emit(
        r#"
class Markdowner
  def render(text)
    text
  end
end
"#,
    );
    assert!(!out.contains("def self.render"), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn native_keyword_mailer_stays_unwrapped() {
    // The mailer pass refuses a native keyword wrapper: forwarding
    // `urgent` positionally would mis-bind. The source signature stays
    // `urgent: false`; no positional wrapper is invented.
    let (out, diags) = lower_and_emit(
        r#"
class Notifier < ActionMailer::Base
  def notify(user, urgent: false)
    mail(to: user)
  end
end
"#,
    );
    assert!(out.contains("def notify(user, urgent: false)"),
        "native keyword declaration was flattened:\n{out}");
    assert!(!out.contains("def self.notify"), "keyword mailer gained a class-side wrapper:\n{out}");
    assert!(
        !out.contains("urgent = false") && !out.contains("new.notify(user, urgent)"),
        "positional keyword wrapper was invented:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one residue note: {diags:?}");
    assert!(diags[0].message.contains("keyword"),
        "residue should name the keyword boundary: {diags:?}");
}

#[test]
fn block_taking_method_stays_unwrapped_with_residue() {
    let (out, diags) = lower_and_emit(
        r#"
class Notifier < ActionMailer::Base
  def notify(user, &formatter)
    mail(to: user)
  end
end
"#,
    );
    assert!(
        !out.contains("def self.notify"),
        "a block cannot be forwarded by the wrapper:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one residue note: {diags:?}");
    assert!(diags[0].message.contains("block"), "{:?}", diags[0]);
}

#[test]
fn initialize_is_never_wrapped() {
    let (out, _) = lower_and_emit(
        r#"
class Notifier < ActionMailer::Base
  def initialize
    super
  end

  def notify(user)
    mail(to: user)
  end
end
"#,
    );
    assert!(!out.contains("def self.initialize"), "{out}");
    assert!(out.contains("def self.notify"), "{out}");
}
