//! Where the `Oj.mimic_JSON` stand-in is allowed to ship.
//!
//! Ingest installs one Ruby class (`ingest::oj`) whose `generate` is
//! `JSON.generate` and whose `load` is the one-argument `JSON.parse`.
//! Ruby, JRuby and Spinel emit that class and run it. Every other
//! transpile target either drops the class (Python, Crystal) or refuses
//! an incidental call inside it (`String#read` on TypeScript, the `JSON`
//! constant on Rust and the other strict targets). Both are the wrong
//! program: a call with nothing behind it, or an error that is not about
//! Oj. This pass is the one gate. It drops the class, then rewrites each
//! `Oj.*` call and each `Oj` / `Oj::…` constant on the emit-body walk
//! into a `raise`, and records one unsupported diagnostic so a clean
//! check cannot claim the reference.

use crate::App;
use crate::build_target::BuildTarget;
use crate::dialect::LibraryClassOrigin;

/// Targets that emit the stand-in and execute it as Ruby.
pub fn stand_in_runs(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel
    )
}

/// Blog, Roda and Futamura keep the app's own call: they run Ruby (the
/// real gem, or residual Rails), not a transpile that would drop the class.
fn keeps_source_call(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Blog | BuildTarget::Roda | BuildTarget::Futamura
    )
}

/// `None` when `app` is already what `target` should emit. `Some` is a
/// copy with the stand-in removed and each `Oj` call or constant rewritten.
pub fn without_unrun_stand_in(app: &App, target: BuildTarget) -> Option<App> {
    if stand_in_runs(target) || keeps_source_call(target) {
        return None;
    }
    if !app
        .library_classes
        .iter()
        .any(|lc| matches!(lc.origin, Some(LibraryClassOrigin::Oj)))
    {
        return None;
    }
    let mut app = app.clone();
    // Drop the class before the walk so its own `JSON.parse` is not refused,
    // and so a later emit cannot write the class the calls no longer reach.
    app.library_classes
        .retain(|lc| !matches!(lc.origin, Some(LibraryClassOrigin::Oj)));
    crate::lower::gem_refusal::refuse_references(
        &mut app,
        target.as_str(),
        &crate::lower::gem_refusal::Refusal {
            root: "Oj",
            construct: "Oj",
            why: "lowers to the Ruby stand-in, which this target does not emit. \
                  The reference is refused instead of being written against a missing class",
            raised: "oj stand-in is not emitted on this target",
        },
    );
    Some(app)
}

