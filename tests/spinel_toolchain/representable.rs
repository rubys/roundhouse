//! The native half of `emit_and_run::representable`: the same decorators
//! compile with Spinel and render the same JSON text.

use super::representable_decorator;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_representable_decorator_renders_natively() {
    let run = representable_decorator::overlay().run_spinel(&format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        representable_decorator::ASSERTIONS
    ));
    run.assert_passes();
    assert!(run.stdout.contains("Representable decorator OK"), "{}", run.stdout);
}
