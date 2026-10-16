//! `ActionCable.server.broadcast` payload encoding on the CRuby overlay
//! (#619); the native twin is `spinel_toolchain/cable_broadcast_json.rs`.

use super::cable_broadcast_json_contract as contract;

/// A broadcast payload is written as Rails writes it: every value as
/// JSON, HTML characters in strings escaped.
#[test]
fn a_broadcast_payload_is_written_as_rails_writes_it() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
