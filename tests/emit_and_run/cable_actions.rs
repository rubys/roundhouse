//! `subscription.perform` on the CRuby overlay (#71 item 6): the shared
//! contract in `support/cable_actions.rs`, run through the overlay's own
//! `Channel::Base#perform_action`. The native twin is
//! `spinel_toolchain/cable_actions.rs`.

use super::cable_actions_contract as contract;

/// A client's `subscription.perform(action, data)` runs the channel's
/// action with Rails' own rules: which methods are actions, which get
/// `data`, `receive` as the default, JSON `false` kept false, nothing for a
/// private or unknown name or a rejected subscription, and the modules an
/// initializer mixes into the channel.
#[test]
fn a_cable_action_runs_with_rails_rules() {
    let run = contract::overlay().run_ruby(&ruby_script());
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}

/// The CRuby overlay builds a channel by its registry, as its
/// `Cable::Connection` does for a subscribe frame.
fn ruby_script() -> String {
    format!(
        r#"require "json"
FRAMES_JSON = {frames:?}
def build_channel(name)
  identifier = JSON.generate("channel" => name)
  klass = ActionCable::Channel::Base.lookup(name)
  channel = klass.new(nil, identifier, ActionCable::Channel::Parameters.new(JSON.parse(identifier)))
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
