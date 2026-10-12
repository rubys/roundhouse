use super::{array_excluding, emit_and_run};

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn array_excluding_and_without_run_natively() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/array_excluding_probe.rb",
            array_excluding::SOURCE,
        )
        .run_spinel(array_excluding::ASSERTIONS);
    run.assert_passes();
    assert!(
        run.stdout
            .contains("Array excluding emitted contract passed")
    );
}
