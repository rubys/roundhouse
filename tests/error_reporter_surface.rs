//! `Rails.error` and `Rails.event` with subscribers, against native
//! activesupport as the oracle: an error report reaches a subscriber with
//! its handled flag, severity, context and source
//! (active_support/error_reporter.rb); `handle`, `record`, `unexpected`,
//! `disable`, middleware and the reported-once rule; an event reaches a
//! subscriber with its name, filtered payload, tags and context
//! (active_support/event_reporter.rb). `RH_RAILS_ORACLE=<version>` pins the
//! activesupport the native control loads.
//!
//! A report to an error reporter with no subscriber is not here: Rails
//! drops it and the runtime logs it (`rails_error_report_logs_at_its_severity`
//! in tests/emit_and_run/active_support_extensions.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

/// App code, written once and run both natively and emitted.
const SURFACE: &str = r####"module ReporterSurface
  class ErrorSink
    attr_reader :seen

    def initialize
      @seen = []
    end

    def report(error, handled:, severity:, context:, source:)
      @seen << "#{error.class}: #{error.message} handled=#{handled} severity=#{severity} source=#{source} context=#{context.inspect}"
    end
  end

  class Exploding
    def report(error, handled:, severity:, context:, source:)
      raise "sink failed"
    end
  end

  class EventSink
    attr_reader :seen

    def initialize
      @seen = []
    end

    def emit(event)
      @seen << "#{event[:name]} payload=#{event[:payload].inspect} tags=#{event[:tags].inspect} context=#{event[:context].inspect}"
    end
  end

  def self.outcome
    yield.inspect
  rescue Exception => e
    "#{e.class}: #{e.message.lines.first.to_s.strip}"
  end

  def self.error_report
    out = []
    sink = ErrorSink.new
    Rails.error.subscribe(sink)
    out << "report default: #{Rails.error.report(RuntimeError.new("a")).inspect}"
    Rails.error.report(RuntimeError.new("b"), handled: false)
    Rails.error.report(RuntimeError.new("c"), handled: true, severity: :info, context: { id: 1 }, source: "job")
    Rails.error.set_context(request: "r1") { Rails.error.report(RuntimeError.new("d"), context: { user: 2 }) }
    Rails.error.report(RuntimeError.new("e"))
    out << "severity: #{outcome { Rails.error.report(RuntimeError.new("f"), severity: :debug) }}"
    out << "not an exception: #{outcome { Rails.error.report("text") }}"
    repeated = RuntimeError.new("g")
    Rails.error.report(repeated)
    Rails.error.report(repeated)
    out << "handle: #{outcome { Rails.error.handle(ArgumentError, fallback: -> { :fallback }) { raise ArgumentError, "h" } }}"
    out << "handle other: #{outcome { Rails.error.handle(ArgumentError) { raise TypeError, "i" } }}"
    out << "record: #{outcome { Rails.error.record(source: "rec") { raise IOError, "j" } }}"
    out << "unexpected: #{outcome { Rails.error.unexpected("k") }}"
    Rails.error.disable(sink) { Rails.error.report(RuntimeError.new("l")) }
    Rails.error.add_middleware(->(error, context:, handled:, severity:, source:) { context.merge(mw: error.message) })
    Rails.error.report(RuntimeError.new("m"))
    out << "bad subscriber: #{outcome { Rails.error.subscribe(Object.new) }}"
    exploding = Exploding.new
    Rails.error.subscribe(exploding)
    out << "raising subscriber: #{outcome { Rails.error.report(RuntimeError.new("n")) }}"
    Rails.error.unsubscribe(exploding)
    Rails.error.unsubscribe(sink)
    out.concat(sink.seen)
    out
  end

  def self.event_report
    out = []
    ActiveSupport.filter_parameters << :password
    sink = EventSink.new
    only_users = EventSink.new
    Rails.event.subscribe(sink)
    Rails.event.subscribe(only_users) { |event| event[:name].start_with?("user.") }
    out << "notify: #{Rails.event.notify("user.created", id: 1, password: "secret").inspect}"
    Rails.event.notify("order.paid", { total: 5 })
    Rails.event.tagged("web", request: "r2") { Rails.event.notify("tagged.event", ok: true) }
    Rails.event.set_context(tenant: "t1")
    Rails.event.notify("with.context")
    Rails.event.clear_context
    out << "bad subscriber: #{outcome { Rails.event.subscribe(Object.new) }}"
    Rails.event.unsubscribe(sink)
    Rails.event.unsubscribe(only_users)
    out << "filtered: #{only_users.seen.inspect}"
    out.concat(sink.seen)
    out
  end

  def self.report
    (error_report + event_report).join("\n")
  end
end
"####;

const EXPECTED: &str = r####"report default: nil
severity: ArgumentError: severity must be one of :error, :warning, :info, got: :debug
not an exception: ArgumentError: Reported error must be an Exception, got: "text"
handle: :fallback
handle other: TypeError: i
record: IOError: j
unexpected: nil
bad subscriber: ArgumentError: Error subscribers must respond to #report
raising subscriber: RuntimeError: sink failed
RuntimeError: a handled=true severity=warning source=application context={}
RuntimeError: b handled=false severity=error source=application context={}
RuntimeError: c handled=true severity=info source=job context={id: 1}
RuntimeError: d handled=true severity=warning source=application context={request: "r1", user: 2}
RuntimeError: e handled=true severity=warning source=application context={request: nil}
RuntimeError: g handled=true severity=warning source=application context={request: nil}
ArgumentError: h handled=true severity=warning source=application context={request: nil}
IOError: j handled=false severity=error source=rec context={request: nil}
RuntimeError: k handled=true severity=warning source=application context={request: nil}
RuntimeError: m handled=true severity=warning source=application context={request: nil, mw: "m"}
RuntimeError: n handled=true severity=warning source=application context={request: nil, mw: "n"}
notify: nil
bad subscriber: ArgumentError: Event subscriber Object must respond to #emit
filtered: ["user.created payload={id: 1, password: \"[FILTERED]\"} tags={} context={}"]
user.created payload={id: 1, password: "[FILTERED]"} tags={} context={}
order.paid payload={total: 5} tags={} context={}
tagged.event payload={ok: true} tags={web: true, request: "r2"} context={}
with.context payload=nil tags={} context={tenant: "t1"}
"####;

#[test]
fn native_rails_answers() {
    let pin = std::env::var("RH_RAILS_ORACLE").unwrap_or_else(|_| "8.1.4".to_string());
    let output = Command::new("ruby")
        .args(["-e"])
        .arg(format!(
            "gem \"activesupport\", \"{pin}\"\nrequire \"active_support\"\nrequire \"active_support/event_reporter\"\nrequire \"active_support/error_reporter\"\nrequire \"active_support/execution_context\"\nrequire \"active_support/core_ext/hash/keys\"\nrequire \"active_support/core_ext/object/deep_dup\"\nmodule Rails\n  def self.error = ActiveSupport.error_reporter\n  def self.event = ActiveSupport.event_reporter\nend\n{SURFACE}\nputs ReporterSurface.report\n"
        ))
        .output()
        .expect("native Ruby control");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}

#[test]
fn emitted_runtime_matches_native_rails() {
    let run = emit_and_run::real_blog()
        .write("app/lib/reporter_surface.rb", SURFACE)
        .run_ruby("puts ReporterSurface.report\n");
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
}
