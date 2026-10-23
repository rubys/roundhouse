//! Safe navigation compiled natively; the CRuby twin is
//! `emit_and_run/safe_navigation.rs`.

use super::safe_navigation_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn safe_navigation_guards_only_nil_natively() {
    let run = contract::overlay().run_spinel(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
