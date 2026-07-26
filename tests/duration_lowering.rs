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
fn temporal_predicates_ground_to_comparisons() {
    // AS's `after?`/`before?` are `self > other` / `self < other`; no
    // transpiled runtime can install them on Time, so they ground here.
    let out = lower_and_emit(
        r#"
class Window
  def commentable?(t)
    t.after?(30.days.ago)
  end

  def closing?(t)
    (t - 1.hour).before?(30.days.ago)
  end
end
"#,
    );
    assert!(
        out.contains("t > ActiveSupport::Duration.days(30).ago"),
        "after? must ground to `>`:\n{out}",
    );
    assert!(
        out.contains("t - ActiveSupport::Duration.hour(1).to_i < ActiveSupport::Duration.days(30).ago"),
        "before? must ground to `<`, and Time - Duration to its seconds \
         (`-` binds tighter than `<`, so the parens are redundant):\n{out}",
    );
}

#[test]
fn temporal_predicates_stand_down_when_the_app_defines_them() {
    let out = lower_and_emit(
        r#"
class Slot
  def after?(other)
    ends_at > other.starts_at
  end

  def overlaps?(t)
    t.after?(3.days.ago)
  end
end
"#,
    );
    assert!(
        out.contains("t.after?(ActiveSupport::Duration.days(3).ago)"),
        "an app-defined after? disables the rewrite wholesale:\n{out}",
    );
}

#[test]
fn time_minus_duration_grounds_to_seconds() {
    // `Time - Duration` is what the CRuby overlay's Time reopen unwraps;
    // a Duration RECEIVER is Duration arithmetic and stays untouched.
    let out = lower_and_emit(
        r#"
class Cutoff
  def shifted(t)
    t - 1.hour
  end

  def widened
    2.days - 1.hour
  end
end
"#,
    );
    assert!(
        out.contains("t - ActiveSupport::Duration.hour(1).to_i"),
        "Time - Duration must ground to seconds:\n{out}",
    );
    assert!(
        out.contains("ActiveSupport::Duration.days(2) - ActiveSupport::Duration.hour(1)"),
        "Duration - Duration must stay untouched:\n{out}",
    );
}

#[test]
fn time_compared_against_a_bare_duration_takes_rails_coercion() {
    // Rails answers this comparison through `Time#<=>` →
    // `to_datetime <=> other` → `Duration#coerce`, i.e. astronomical
    // Julian day vs seconds — `Time.now <= 1.hour` is FALSE. Grounding
    // to bare seconds would answer a different question and raise on
    // CRuby, so the epoch offset has to survive.
    let out = lower_and_emit(
        r#"
class Story
  def send_referrer?
    t = Time.now.utc
    t <= 1.hour
  end
end
"#,
    );
    assert!(
        out.contains("t.to_f + 210866760000.0 <= ActiveSupport::Duration.hour(1).to_f * 86400.0"),
        "Time vs bare Duration keeps Rails' ajd-vs-seconds arithmetic:\n{out}",
    );
}

#[test]
fn numeric_vs_duration_still_grounds_to_seconds() {
    // The elapsed-seconds shape (`Time - Time`) is genuinely numeric and
    // keeps the plain `.to_f` grounding — the Time-vs-Duration rule must
    // not swallow it.
    let out = lower_and_emit(
        r#"
class Flag
  def flaggable?(created_at)
    Time.now.utc - created_at <= 30.days
  end
end
"#,
    );
    assert!(
        out.contains("<= ActiveSupport::Duration.days(30).to_f"),
        "numeric vs Duration grounds to seconds:\n{out}",
    );
    assert!(
        !out.contains("210866760000.0"),
        "the elapsed-seconds shape must NOT take the ajd rewrite:\n{out}",
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
