//! A `**kwargs` the shared keyword lowering makes positional (legacy
//! policy: a dynamic `send`, an ordinary callee) stays a keyword splat in
//! emitted Ruby. In Ruby 3 a positional Hash does not bind keyword
//! parameters, so `@connection.send(meth, *args, kwargs)` raised where the
//! source's `**kwargs` worked.
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
fn native_and_emitted_ruby_keep_a_forwarded_keyword_splat() {
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
    run.assert_passes();
    assert_eq!(run.stdout, "keyword splat passed\n");
    let emitted =
        std::fs::read_to_string(run.emitted.join("app/models/keyword_connection_proxy.rb")).unwrap();
    assert!(emitted.contains("@connection.send(meth, *args, **kwargs, &block)"), "{emitted}");
    assert!(emitted.contains("**options)"), "{emitted}");
}
