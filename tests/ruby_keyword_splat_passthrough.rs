//! A `**kwargs` the shared keyword lowering makes positional (legacy
//! policy: a dynamic `send`, an ordinary callee) stays a keyword splat in
//! emitted Ruby. In Ruby 3 a positional Hash does not bind keyword
//! parameters, so `@connection.send(meth, *args, kwargs)` raised where the
//! source's `**kwargs` worked.
//!
//! The untyped `@connection.query` relay still has an unverified keyword
//! ABI. Its retained refusal is part of this contract: matching this Ruby
//! execution does not establish support for every possible receiver.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

const LIBRARY: &str = r##"class KeywordConnection
  def query(sql, limit:, offset: 0)
    "#{sql} #{limit} #{offset}"
  end
end

class KeywordConnectionProxy
  def initialize(connection)
    @connection = connection
  end

  def method_missing(meth, *args, **kwargs, &block)
    @connection.send(meth, *args, **kwargs, &block)
  end

  def respond_to_missing?(meth, include_private = false)
    @connection.respond_to?(meth, include_private) || super
  end

  def relay(options)
    @connection.query("relay", **options)
  end
end
"##;

const ASSERTIONS: &str = r##"
proxy = KeywordConnectionProxy.new(KeywordConnection.new)
got = proxy.query("q", limit: 5, offset: 2)
raise "method_missing: #{got.inspect}" unless got == "q 5 2"
got = proxy.relay({ limit: 3 })
raise "relay: #{got.inspect}" unless got == "relay 3 0"
puts "keyword splat passed"
"##;

#[test]
fn an_unverified_relay_keeps_its_refusal_and_ruby_keyword_packet() {
    let native = Command::new("ruby")
        .arg("-e")
        .arg(format!("{LIBRARY}\n{ASSERTIONS}"))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), "keyword splat passed\n");

    let run = emit_and_run::real_blog()
        .write("app/lib/keyword_connection_proxy.rb", LIBRARY)
        .run_ruby(ASSERTIONS);
    assert!(!run.errors.is_empty(), "the untyped relay must not earn ABI admission");
    assert!(run.errors.iter().all(|error|
        error.contains("keyword splat into source keyword parameters not supported")
            && error.contains("keyword destination's native argument ABI cannot be verified")),
        "unexpected diagnostic beside the unverified relay: {:?}", run.errors);
    assert!(run.success, "emitted Ruby failed: {}", run.stderr);
    assert_eq!(run.stdout, String::from_utf8_lossy(&native.stdout), "emitted vs native");
    let emitted =
        std::fs::read_to_string(run.emitted.join("app/models/keyword_connection_proxy.rb")).unwrap();
    assert!(emitted.contains("@connection.send(meth, *args, **kwargs, &block)"), "{emitted}");
    assert!(emitted.contains("**options)"), "{emitted}");
}
