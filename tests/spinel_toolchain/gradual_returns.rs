use super::gradual_returns_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn gradual_room_and_json_values_cross_native_rbs_boundaries() {
    let run = contract::overlay().run_spinel_with_rbs(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "{}", run.stderr);
}
