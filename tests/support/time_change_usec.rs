pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog().write(
        "app/models/clock_probe.rb",
        r#"class ClockProbe
  CALLS = []

  def self.normalize(at: Time.current)
    at.change(usec: at.usec)
  end

  def self.with_usec(at: Time.current, value: 0)
    at.change(usec: value)
  end

  def self.source
    CALLS << "time"
    Time.utc(2026, 10, 26, 1, 2, 3)
  end

  def self.microseconds
    CALLS << "usec"
    42
  end

  def self.ordered
    source.change(usec: microseconds)
  end

  def self.custom
    CustomClock.new.change(usec: 41)
  end
end

class CustomClock
  def change(usec: 0)
    usec + 1
  end
end
"#,
    )
}

pub const SCRIPT: &str = r##"
times = [
  Time.at(1, 123456789, :nsec).getutc,
  Time.at(-1, 987654321, :nsec).getutc,
  Time.at(1, 123456789, :nsec).getlocal(19800),
  Time.at(1, 123456789, :nsec).getlocal(0)
]
times.each do |time|
  original = time.nsec
  normalized = ClockProbe.normalize(at: time)
  changed = ClockProbe.with_usec(at: time, value: 654321)
  raise "receiver mutated" unless time.nsec == original
  raise "epoch changed" unless normalized.to_i == time.to_i && changed.to_i == time.to_i
  raise "offset changed" unless changed.utc_offset == time.utc_offset
  raise "UTC flag changed" unless changed.utc? == time.utc?
  puts "time #{normalized.nsec} #{changed.nsec} #{changed.utc_offset} #{changed.utc?}"
end
negative = ClockProbe.with_usec(at: Time.utc(2026, 10, 26, 1, 2, 3), value: -1)
raise "negative microseconds" unless negative.sec == 2 && negative.usec == 999999
begin
  ClockProbe.with_usec(at: Time.utc(2026), value: 1000000)
  raise "out-of-range value accepted"
rescue ArgumentError => error
  raise error.message unless error.message == "argument out of range"
end
ordered = ClockProbe.ordered
raise "evaluation order" unless ClockProbe::CALLS == ["time", "usec"]
raise "ordered microseconds" unless ordered.usec == 42
local = Time.at(1730615400, 123456789, :nsec).getlocal
changed = ClockProbe.with_usec(at: local, value: 42)
raise "local zone changed" unless changed.zone == local.zone && changed.dst? == local.dst?
raise "local offset changed" unless changed.utc_offset == local.utc_offset
raise "local instant changed" unless changed.to_i == local.to_i && changed.nsec == 42000
raise "custom change was rewritten" unless ClockProbe.custom == 42
puts "time changes passed"
"##;

pub const EXPECTED: &str = "time 123456000 654321000 0 true\ntime 987654000 654321000 0 true\ntime 123456000 654321000 19800 false\ntime 123456000 654321000 0 false\ntime changes passed\n";
