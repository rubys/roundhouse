use super::{array_excluding, emit_and_run};

#[test]
fn array_excluding_and_without_run_in_emitted_ruby() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/array_excluding_probe.rb",
            array_excluding::SOURCE,
        )
        .run_ruby(array_excluding::ASSERTIONS);
    run.assert_passes();
    assert!(
        run.stdout
            .contains("Array excluding emitted contract passed")
    );
}
