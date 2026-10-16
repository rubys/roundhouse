//! Class-side parameter defaults compiled natively; the CRuby twin is
//! `emit_and_run/class_side_default.rs`.

use super::class_side_default_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_class_methods_default_reads_a_class_method_natively() {
    let run = contract::overlay().run_spinel(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
