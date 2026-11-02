//! The native half of `emit_and_run::stdlib_rexml_include`.

use super::stdlib_rexml_include_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn rexml_classes_named_through_include_run_natively() {
    let run = contract::app().run_spinel_slim(&[], contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("rexml passed"), "{}", run.stdout);
}
