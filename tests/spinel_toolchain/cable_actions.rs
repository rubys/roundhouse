//! `subscription.perform` on Spinel (#71 item 6): the shared contract in
//! `support/cable_actions.rs`, compiled natively and run through the
//! generated `ActionCable::Channel.perform` arms, which no fixture reaches.
//! The overlay twin is `emit_and_run/cable_actions.rs`.

use super::cable_actions_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_cable_action_runs_with_rails_rules_natively() {
    let run = contract::overlay().run_spinel(&spinel_script());
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}

/// Spinel builds it through the generated `ActionCable::Channel.build`,
/// as `Cable.subscribe` does.
fn spinel_script() -> String {
    format!(
        r#"require "json"
FRAMES_JSON = {frames:?}
def build_channel(name)
  identifier = "{{\"channel\":\"" + name + "\"}}"
  channel = ActionCable::Channel.build(name, nil, identifier)
  channel.subscribed
  channel
end
probe = build_channel("ProbeChannel")
rejected = build_channel("RejectingProbeChannel")
{drive}"#,
        frames = contract::FRAMES,
        drive = contract::DRIVE,
    )
}
