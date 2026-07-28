//! `errors.full_messages` identity fold
//! (`lower::apply_errors_full_messages_lowering`).
//!
//! Shape tests over the fold: the framework runtime's `errors` IS the
//! array of full-message strings, so the extra hop collapses to its
//! receiver — on both a self-receiver (`errors.full_messages`, a model
//! reading its own) and a foreign one (`record.errors.full_messages`,
//! a controller reading another object's). The pass stands down when
//! the app defines its own `full_messages`.

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_errors_full_messages_lowering;
use roundhouse::App;

fn lower_and_emit(source: &str) -> String {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    apply_errors_full_messages_lowering(&mut app);
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn self_receiver_hop_collapses() {
    let out = lower_and_emit(
        r#"
class Draft
  def summary
    errors.full_messages.join(", ")
  end
end
"#,
    );
    assert!(!out.contains("full_messages"), "hop should be folded away:\n{out}");
    assert!(
        out.contains(r#"errors.join(", ")"#),
        "expected the fold to leave the errors receiver in place:\n{out}"
    );
}

#[test]
fn foreign_receiver_keeps_its_receiver() {
    // The receiver of `errors` is unconstrained and must survive the
    // fold — dropping it would silently retarget the read to self.
    let out = lower_and_emit(
        r#"
class Draft
  def report(record)
    record.errors.full_messages
  end
end
"#,
    );
    assert!(!out.contains("full_messages"), "hop should be folded away:\n{out}");
    assert!(
        out.contains("record.errors"),
        "expected the foreign receiver preserved:\n{out}"
    );
}

#[test]
fn app_defined_full_messages_stands_the_pass_down() {
    // If the app owns the name, the call is not Rails' and folding it
    // would delete a real dispatch.
    let out = lower_and_emit(
        r#"
class Draft
  def full_messages
    ["custom"]
  end

  def summary
    errors.full_messages
  end
end
"#,
    );
    assert!(
        out.contains("errors.full_messages"),
        "pass should stand down when the app defines full_messages:\n{out}"
    );
}

#[test]
fn unrelated_full_messages_receiver_is_untouched() {
    // Only an `errors` reader is our accumulator; a same-named method
    // on anything else keeps its call.
    let out = lower_and_emit(
        r#"
class Draft
  def summary(report)
    report.full_messages
  end
end
"#,
    );
    assert!(
        out.contains("report.full_messages"),
        "non-errors receiver should be left alone:\n{out}"
    );
}
