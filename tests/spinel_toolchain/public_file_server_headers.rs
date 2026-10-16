use super::public_file_server_headers_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn public_file_server_headers_follow_the_apps_config_natively() {
    let run = contract::overlay().run_spinel(&contract::script());
    run.assert_passes();
    assert!(run.stdout.contains("public file server headers contract passed"));
}
