use super::cable_allowed_origins_contract as contract;

#[test]
fn cable_allowed_origins_follow_the_apps_config_in_emitted_ruby() {
    let run = contract::overlay().run_ruby(&contract::script());
    run.assert_passes();
    assert!(run.stdout.contains("Action Cable origin contract passed"));
}
