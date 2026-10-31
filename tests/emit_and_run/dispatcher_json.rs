use super::{dispatcher_json_contract as contract, emit_and_run};

fn observations(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON response observation"))
        .collect()
}

#[test]
fn callback_and_rescue_json_responses_run() {
    let run = contract::app().run_ruby(contract::PROGRAM);
    run.assert_passes();
    assert_eq!(observations(&run.stdout), observations(contract::EXPECTED));
}

#[test]
fn callback_and_rescue_json_responses_match_rails() {
    let helper =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let mut script = format!(
        "require {:?}\nexit 77 unless RailsOracle.available?(\"actionpack\")\n\
         RailsOracle.activate(\"actionpack\")\nrequire \"action_controller\"\n\
         require \"action_controller/api\"\nrequire \"rack/mock\"\n",
        helper.to_str().unwrap(),
    );
    for (path, source) in contract::SOURCES {
        if path.starts_with("app/") {
            script.push_str(source);
        }
    }
    script.push_str(contract::PROGRAM);
    let oracle = emit_and_run::ruby()
        .args(["-e", &script])
        .output()
        .expect("Rails oracle");
    if oracle.status.code() == Some(77) {
        eprintln!("skipping dispatcher JSON oracle: actionpack for RH_RAILS_ORACLE is unavailable");
        return;
    }
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let expected = String::from_utf8(oracle.stdout).unwrap();
    assert_eq!(observations(&expected), observations(contract::EXPECTED));
    let run = contract::app().run_ruby(contract::PROGRAM);
    run.assert_passes();
    assert_eq!(observations(&run.stdout), observations(&expected));
}
