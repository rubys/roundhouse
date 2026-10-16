//! The native half of `emit_and_run::stdlib_rnp`: the same constants
//! compile with Spinel and answer what plain Ruby answers.

use super::stdlib_rnp_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn stdlib_constants_an_api_uses_execute_natively() {
    let run = contract::overlay().run_spinel(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("stdlib constants OK"), "{}", run.stdout);
}
