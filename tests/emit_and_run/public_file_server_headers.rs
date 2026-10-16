use super::public_file_server_headers_contract as contract;

#[test]
fn public_file_server_headers_follow_the_apps_config_in_emitted_ruby() {
    let run = contract::overlay().run_ruby(&contract::script());
    run.assert_passes();
    assert!(run.stdout.contains("public file server headers contract passed"));
}
