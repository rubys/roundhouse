//! A render inherits charset from response state; head and send_data suppress it.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[path = "support/response_content_type.rs"]
mod contract;
use contract::{EXPECTED, PROGRAM};

fn observations(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("response header observations")
}

fn emitted() -> serde_json::Value {
    let run = contract::app().run_ruby(&format!(
        "require File.expand_path(\"app/controllers/content_probes_controller\", Dir.pwd)\n{PROGRAM}"));
    run.assert_passes();
    observations(&run.stdout)
}

#[test]
fn response_content_types_run_without_rails() {
    assert_eq!(emitted(), observations(EXPECTED));
}

#[test]
fn response_content_types_match_rails_when_available() {
    let oracle = emit_and_run::ruby()
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/support/response_content_type/oracle.rb"),
        )
        .output()
        .expect("Rails response header oracle");
    if oracle.status.code() == Some(77) {
        eprintln!("skipping response content-type oracle: RailsOracle actionpack is unavailable");
        return;
    }
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let expected = observations(&String::from_utf8(oracle.stdout).unwrap());
    assert_eq!(expected, observations(EXPECTED));
    assert_eq!(emitted(), expected);
}
