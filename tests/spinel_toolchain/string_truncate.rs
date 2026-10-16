use super::{emit_and_run, string_truncate};

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn string_truncate_runs_natively() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/string_truncate_probe.rb",
            string_truncate::SOURCE,
        )
        .run_spinel(string_truncate::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("String truncate emitted contract passed"));
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/string_truncate_probe.rb"))
        .expect("emitted probe class");
    assert!(emitted.contains("ActiveSupport.truncate("), "{emitted}");
    assert_eq!(emitted.matches(".truncate(").count(), emitted.matches("ActiveSupport.truncate(").count(), "every String#truncate is grounded:\n{emitted}");
}
