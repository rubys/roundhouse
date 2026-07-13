//! ActiveSupport duration grounding (`lower::apply_duration_lowering`).
//!
//! Shape tests over the shared post-analyze rewrite: `<n>.days` and
//! friends ground to `ActiveSupport::Duration.days(<n>)` wherever they
//! appear in a hook body, plural units ground even on untyped
//! receivers, and the colliding singulars (`day`/`hour`/`month`/
//! `year`) ground only on provably numeric receivers so `Time`
//! component readers keep their dispatch. Hook ORDER (duration after
//! send_dispatch, grounding the synthesized plural arms) is locked by
//! `tests/send_dispatch_lowering.rs`; the model-schema and
//! view-vestige paths by `tests/lowered_ruby_emit.rs`.

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_duration_lowering;
use roundhouse::App;

fn lower_and_emit(source: &str) -> String {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    apply_duration_lowering(&mut app);
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn plural_units_ground_unconditionally() {
    let out = lower_and_emit(
        r#"
class Window
  def recent?(t)
    t > 3.days.ago && t > w.weeks.ago
  end
end
"#,
    );
    assert!(
        out.contains("ActiveSupport::Duration.days(3).ago"),
        "literal plural must ground:\n{out}",
    );
    assert!(
        out.contains("ActiveSupport::Duration.weeks(w).ago"),
        "plural grounds even on an untyped receiver:\n{out}",
    );
}

#[test]
fn colliding_singular_grounds_only_on_numeric_receivers() {
    let out = lower_and_emit(
        r#"
class Cutoff
  def stale?(t)
    t < 1.hour.ago
  end

  def stonewall?(time)
    time.month == 6 && time.day == 28
  end
end
"#,
    );
    assert!(
        out.contains("ActiveSupport::Duration.hour(1).ago"),
        "Int-literal singular must ground:\n{out}",
    );
    assert!(
        out.contains("time.month == 6") && out.contains("time.day == 28"),
        "Time component readers must keep their dispatch:\n{out}",
    );
    assert!(
        !out.contains("Duration.month(time)") && !out.contains("Duration.day(time)"),
        "component readers must NOT be rewritten:\n{out}",
    );
}

#[test]
fn non_colliding_singulars_ground_untyped() {
    // minute/second/week/fortnight have no Time-reader collision, so the
    // singular grounds even when the receiver's type is unresolved.
    let out = lower_and_emit(
        r#"
class Ttl
  def cache_time(n)
    n.minute
  end
end
"#,
    );
    assert!(
        out.contains("ActiveSupport::Duration.minute(n)"),
        "non-colliding singular grounds on an untyped receiver:\n{out}",
    );
}
