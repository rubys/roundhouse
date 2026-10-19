//! Conditions, `!`, value-position `||` / `&&` and predicate blocks on the
//! TypeScript emit test values as Ruby does (run against CRuby in
//! `typescript_toolchain::real_blog_ruby_truthiness_runs_under_tsx`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/truthiness.rs"]
mod truthiness;

use roundhouse::project::BuildTarget;

#[test]
fn ruby_truthiness_emits_without_errors() {
    let (emitted, errors) = truthiness::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let ts = std::fs::read_to_string(emitted.join("app/models/truth_probe.ts")).unwrap();
    // A boolean condition stays as written; a number or string is tested against nil only.
    assert!(ts.contains("no ? \"t\" : \"f\""), "{ts}");
    assert!(ts.contains("zero != null ? \"t\" : \"f\""), "{ts}");
    assert!(ts.contains("zero ?? 5"), "{ts}");
}
