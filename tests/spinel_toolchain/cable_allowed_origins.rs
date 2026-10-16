use super::cable_allowed_origins_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn cable_allowed_origins_follow_the_apps_config_natively() {
    let run = contract::overlay().run_spinel(&contract::script());
    run.assert_passes();
    assert!(run.stdout.contains("Action Cable origin contract passed"));
}
