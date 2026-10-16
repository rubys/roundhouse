//! One Action Cable `perform` contract (#71 item 6) shared by the
//! interpreted (CRuby overlay) and native (spinel) lanes.
//!
//! The expected lines are what actioncable 8.1.4 prints for the same
//! channels and frames: Rails' `action_methods` (public, inherited and
//! mixed-in methods, less `Channel::Base`'s), its arity rule (exactly
//! one parameter gets `data`; `def optional(data = {})` and
//! `def rest(*args)` are called with nothing; `def kw(data, k: 1)` is
//! called with nothing and raises), `receive` for a frame without an
//! action, JSON `false` staying false, and nothing at all for a private
//! method, an unknown action, a `Channel::Base` method or a rejected
//! subscription. Modules a `config/initializers/` file includes into or
//! prepends onto the channel are part of its lookup chain; a public method
//! of the class that a prepended module makes private stays an action and
//! raises NoMethodError when called, as `public_send` does in Rails.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "app/channels/application_cable/channel.rb",
            "module ApplicationCable\n  class Channel < ActionCable::Channel::Base\n  end\nend\n",
        )
        .write(
            "app/channels/probe_actions.rb",
            "module ProbeActions\n  def from_mixin\n    puts \"from_mixin\"\n  end\nend\n",
        )
        .write(
            "app/channels/initializer_actions.rb",
            "module InitializerActions\n  def from_include\n    puts \"from_include\"\n  end\nend\n",
        )
        .write(
            "app/channels/prepended_actions.rb",
            "module PrependedActions\n  def from_prepend\n    puts \"from_prepend\"\n  end\n\n  private\n\n  def shadowed\n    puts \"shadowed (prepended, private)\"\n  end\nend\n",
        )
        // As an app does it in config/initializers: the modules land in the
        // channel's lookup chain after its body was read.
        .write(
            "config/initializers/probe_actions.rb",
            "ProbeChannel.include InitializerActions\nProbeChannel.prepend PrependedActions\n",
        )
        .write(
            "app/channels/app_probe_channel.rb",
            r#"class AppProbeChannel < ApplicationCable::Channel
  def shared_helper
    puts "shared_helper"
  end
end
"#,
        )
        .write(
            "app/channels/probe_channel.rb",
            r#"class ProbeChannel < AppProbeChannel
  include ProbeActions

  def subscribed
    stream_from "probe"
  end

  def one(data)
    puts "one #{data["x"]} #{data["action"]}"
  end

  def none
    puts "none"
  end

  def optional(data = {})
    puts "optional #{data.size}"
  end

  def rest(*args)
    puts "rest #{args.size}"
  end

  def kw(data, k: 1)
    puts "kw"
  end

  def receive(data)
    puts "receive #{data["x"].inspect}"
  end

  def flag(data)
    puts(data["on"] ? "flag on" : "flag off")
  end

  def shadowed
    puts "shadowed (class)"
  end

  private

  def secret(data)
    puts "secret"
  end
end
"#,
        )
        .write(
            "app/channels/rejecting_probe_channel.rb",
            r#"class RejectingProbeChannel < ProbeChannel
  def subscribed
    reject
  end
end
"#,
        )
}

/// The frames, in order; each is the `data` object a client's
/// `subscription.perform(...)` sends.
pub const FRAMES: &str = r#"[
  {"action":"one","x":1}, {"action":"none"}, {"action":"optional","x":2},
  {"action":"rest"}, {"action":"kw","x":3}, {"x":4}, {"action":""},
  {"action":"flag","on":false}, {"action":"flag","on":true}, {"action":"flag"},
  {"action":"secret"}, {"action":"nope"}, {"action":"stream_from"},
  {"action":"shared_helper"}, {"action":"from_mixin"},
  {"action":"from_include"}, {"action":"from_prepend"}, {"action":"shadowed"}
]"#;

/// Drives the frames through `perform_action` on `probe`, then one
/// frame on the rejected subscription `rejected`. Both lanes define
/// those two locals before this runs.
pub const DRIVE: &str = r#"
JSON.parse(FRAMES_JSON).each do |data|
  begin
    probe.perform_action(data)
  rescue ArgumentError
    puts "raised ArgumentError"
  rescue NoMethodError
    puts "raised NoMethodError"
  end
end
rejected.perform_action(JSON.parse("{\"action\":\"one\",\"x\":9}"))
puts "cable actions contract done"
"#;

pub const EXPECTED: &str = "one 1 one
none
optional 0
rest 0
raised ArgumentError
receive 4
receive nil
flag off
flag on
flag off
shared_helper
from_mixin
from_include
from_prepend
raised NoMethodError
cable actions contract done
";
