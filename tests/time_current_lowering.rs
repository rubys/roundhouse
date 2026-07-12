//! `Time.current` grounding (`lower::apply_time_current_lowering`).
//!
//! Shape tests over the shared post-analyze rewrite: the Rails-ism
//! grounds to `Time.now.utc` wherever it appears in a body, and
//! non-`Time` receivers spelling a `current` method keep their
//! dispatch.

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_time_current_lowering;
use roundhouse::App;

fn lower_and_emit(source: &str) -> String {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    apply_time_current_lowering(&mut app);
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn time_current_grounds_to_now_utc() {
    let out = lower_and_emit(
        r#"
class Clock
  def stamp
    Time.current
  end
end
"#,
    );
    assert!(!out.contains("Time.current"), "site should be grounded:\n{out}");
    assert!(out.contains("Time.now.utc"), "expected the grounded form:\n{out}");
}

#[test]
fn time_current_grounds_in_nested_positions() {
    let out = lower_and_emit(
        r#"
class Clock
  def recent?(t)
    t > Time.current.to_i - 60
  end
end
"#,
    );
    assert!(!out.contains("Time.current"), "nested site should be grounded:\n{out}");
    assert!(out.contains("Time.now.utc.to_i"), "chain should ride the grounded form:\n{out}");
}

#[test]
fn non_time_current_keeps_dispatch() {
    let out = lower_and_emit(
        r#"
class Sprint
  def self.current
    "s1"
  end

  def label
    Sprint.current
  end
end
"#,
    );
    assert!(out.contains("Sprint.current"), "app-defined current must keep dispatch:\n{out}");
    assert!(!out.contains("Sprint.now"), "{out}");
}
