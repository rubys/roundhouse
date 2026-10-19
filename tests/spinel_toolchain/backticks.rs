//! Backticks compiled natively; the CRuby twin is
//! `emit_and_run/backticks.rs`.

use super::backticks_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn backticks_run_the_command_natively() {
    let run = contract::overlay().run_spinel(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
