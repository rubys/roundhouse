//! One contract for safe navigation, shared by the interpreted (CRuby
//! overlay) and native (spinel) lanes (rubys/roundhouse#620). `a&.b`
//! evaluates `a` once, and `nil` is the only value it guards: a `false`
//! receiver dispatches (`false&.to_s` is "false"). Ingest desugared it to
//! `a && a.b`, which skipped the call on `false`.
//!
//! The expected lines are CRuby 4.0's output for the same code.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog().write("app/models/safe_navigation_probe.rb", SOURCE)
}

const SOURCE: &str = r##"class SafeNavigationProbe
  attr_reader :calls

  def initialize
    @calls = 0
  end

  # A side effect per call: the receiver of a chain must run once.
  def poll
    @calls += 1
    @calls == 1 ? [@calls] : nil
  end

  def self.chain(probe)
    probe&.poll&.map { |x| x * 10 }
  end

  def self.to_s_of(value)
    value&.to_s
  end

  def self.inspect_of(value)
    value&.inspect
  end

  def self.negate(flag)
    flag&.!
  end
end
"##;

pub const SCRIPT: &str = r##"
probe = SafeNavigationProbe.new
p SafeNavigationProbe.chain(probe)
p probe.calls
p SafeNavigationProbe.chain(nil)
p SafeNavigationProbe.to_s_of(false)
p SafeNavigationProbe.to_s_of(nil)
p SafeNavigationProbe.to_s_of(1)
p SafeNavigationProbe.inspect_of(false)
p SafeNavigationProbe.negate(false)
"##;

pub const EXPECTED: &str = "[10]\n1\nnil\n\"false\"\nnil\n\"1\"\n\"false\"\ntrue\n";
