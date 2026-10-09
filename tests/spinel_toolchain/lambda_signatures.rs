//! The native half of `emit_and_run::lambda_signatures`: the same lambdas
//! compile with Spinel and answer what plain Ruby answers.

use super::lambda_signatures_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn lambdas_with_optional_and_keyword_parameters_run_natively() {
    let run = contract::overlay().run_spinel(&contract::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("lambda signatures OK"), "{}", run.stdout);
}
