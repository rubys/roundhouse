//! GzipCache: identical identity HTML is deflated once.

use std::path::Path;
use std::process::Command;

#[test]
fn identical_bodies_gzip_once() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end

body = "x" * 128
app = lambda { |_env| [200, { "content-type" => "text/html" }, [body]] }
wrapped = GzipCache.wrap(app)
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
a = wrapped.call(env)
b = wrapped.call(env)
raise "status #{a[0]}" unless a[0] == 200
raise "encoding" unless a[1]["content-encoding"] == "gzip"
raise "vary" unless a[1]["vary"].to_s.include?("Accept-Encoding")
raise "body changed" unless a[2] == b[2]
raise "gzipped #{n} times" unless n == 1
raise "not smaller" unless a[2][0].bytesize < body.bytesize

id_env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "identity" }
id = wrapped.call(id_env)
raise "identity encoded" if id[1]["content-encoding"]
raise "identity body" unless id[2] == [body]

head = wrapped.call(env.merge("REQUEST_METHOD" => "HEAD"))
raise "HEAD gzipped" if head[1]["content-encoding"]

no_body = GzipCache.wrap(lambda { |_e| [204, { "content-type" => "text/html" }, ["y" * 128]] })
nb = no_body.call(env)
raise "204 gzipped" if nb[1]["content-encoding"]

q0 = wrapped.call(env.merge("HTTP_ACCEPT_ENCODING" => "gzip;q=0, identity"))
raise "q=0 gzipped" if q0[1]["content-encoding"]
q08 = wrapped.call(env.merge("HTTP_ACCEPT_ENCODING" => "gzip;q=0.8"))
raise "q=0.8 skipped" unless q08[1]["content-encoding"] == "gzip"
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "gzip cache failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn distinct_bodies_do_not_share_a_gzip() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

a_body = "a" * 128
b_body = "b" * 128
a = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [a_body]] })
b = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [b_body]] })
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
ga = a.call(env)
gb = b.call(env)
raise "same gzip" if ga[2][0] == gb[2][0]
raise "a not gzip" unless ga[1]["content-encoding"] == "gzip"
raise "b not gzip" unless gb[1]["content-encoding"] == "gzip"
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "distinct-body gzip cache failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn tep_gzip_cached_hits_on_digest_not_body_pointer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require "digest"
require "zlib"
require_relative "runtime/spinel/tep/tep_core"

n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end

a = "y" * 128
b = "y" * 128
raise "same object" if a.equal?(b)
ga = Tep.gzip_cached(a)
gb = Tep.gzip_cached(b)
raise "gzipped #{n} times" unless n == 1
raise "bodies differ" unless ga == gb
gc = Tep.gzip_cached("z" * 128)
raise "distinct collided" if gc == ga
raise "second body not gzipped" unless n == 2
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "tep gzip cache failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn overlay_read_str_does_not_dup_a_cached_fragment() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/rails_cache"

store = Rails::MemoryStore.new
frag = "message-html" * 32
store.write_str("k", frag, 0)
hit = store.read_str("k")
raise "miss" if hit.nil?
raise "duped" unless hit.equal?(store.read_str("k"))
raise "mutated store" unless hit.frozen?
begin
  hit << "x"
  raise "frozen fragment was mutable"
rescue FrozenError
end
other = store.read("k")
raise "untyped read must still dup" if other.equal?(hit)
other << "x"
raise "store corrupted" unless store.read_str("k") == frag
# write (untyped) also freezes, so a later read_str cannot mutate
# the shared entry — CodeRabbit on #432.
store.write("k2", "plain")
hit2 = store.read_str("k2")
raise "write miss" if hit2.nil?
raise "write not frozen" unless hit2.frozen?
begin
  hit2 << "x"
  raise "write-path fragment was mutable"
rescue FrozenError
end
raise "write store corrupted" unless store.read_str("k2") == "plain"
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "overlay read_str failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
