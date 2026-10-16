//! `ActionCable.server.broadcast` payload encoding compiled natively
//! (#619): `ActionCable.payload_json` on the spinel lane, where a String or
//! nested value used to come out as invalid JSON. The overlay twin is
//! `emit_and_run/cable_broadcast_json.rs`.

use super::cable_broadcast_json_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_broadcast_payload_is_written_as_rails_writes_it_natively() {
    let run = contract::overlay().run_spinel(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
