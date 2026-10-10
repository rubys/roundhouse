//! GzipCache: identical identity HTML is deflated once.

use std::path::Path;
use std::process::Command;

const RAW_DICTIONARY_RESPONSE: &str = r#"
require "zlib"
if ENV["REJECT_RAW_DICTIONARY"] == "1"
  # Reproduce JRuby's raw-stream restriction on the default MRI lane.
  Zlib::Deflate.prepend(Module.new do
    def set_dictionary(_dictionary)
      raise Zlib::StreamError, "raw dictionaries unsupported"
    end
  end)
end
require File.expand_path("runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache", Dir.pwd)

fragment = ("<p>cached fragment</p>" * 100).freeze
env = {"REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip"}
[false, true].each do |with_fragment|
  ["A" * 86, "B" * 86].each do |token|
    raw = ("<p>layout</p>" * 100) + token + ("<p>footer</p>" * 100)
    raw += fragment + ("<p>tail</p>" * 100) if with_fragment
    app = lambda do |_env|
      GzipCache.note_token(token)
      GzipCache.note_fragment(fragment) if with_fragment
      [200, {"content-type" => "text/html"}, [raw]]
    end
    wrapped = GzipCache.wrap(app)
    2.times do
      status, headers, body = wrapped.call(env)
      raise "status" unless status == 200
      raise "encoding" unless headers["content-encoding"] == "gzip"
      raise "round trip" unless Zlib.gunzip(body.join) == raw
    end
  end
end
puts "ALL OK"
"#;

fn check_raw_dictionary_response(interpreter: &str, reject_dictionary: bool) {
    let output = Command::new(interpreter)
        .args(["-e", RAW_DICTIONARY_RESPONSE])
        .env("REJECT_RAW_DICTIONARY", if reject_dictionary { "1" } else { "0" })
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Ruby interpreter available");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "gzip response failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("ALL OK"), "gzip response cases did not execute:\n{stdout}");
}

#[test]
fn gzip_splicing_survives_missing_raw_dictionary_support() {
    check_raw_dictionary_response("ruby", true);
}

#[test]
#[ignore = "requires JRuby 10 and Java 21+"]
fn gzip_splicing_executes_on_jruby() {
    check_raw_dictionary_response("jruby", false);
}

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
fn tep_gzip_cached_repeats_the_last_body_without_a_digest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require "digest"
require "zlib"
require_relative "runtime/spinel/tep/tep_core"

gzips = 0
orig_gzip = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  gzips += 1
  orig_gzip.call(raw)
end
digests = 0
orig_digest = Digest::SHA256.method(:hexdigest)
Digest::SHA256.define_singleton_method(:hexdigest) do |raw|
  digests += 1
  orig_digest.call(raw)
end

def same_body!(gz, raw, what)
  raise "gzip (#{what}) does not inflate to its body" unless Zlib.gunzip(gz) == raw
end

# Nothing served yet: an empty body is not the empty last pair.
same_body!(Tep.gzip_cached(""), "", "empty body")
digests = 0
gzips = 0

# A miss gzips and keeps no copy of the body: a page with a per-request
# token never comes back, and copying it would be wasted.
a = "x" * 200
ga = Tep.gzip_cached(a)
same_body!(ga, a, "first")
raise "first: digest #{digests}, gzip #{gzips}" unless digests == 1 && gzips == 1
raise "a miss kept a copy of the body" unless Tep.instance_variable_get(:@gzip_last_raw).to_s == ""

# Seen again, it is a digest hit and becomes the last body.
raise "digest hit returned other bytes" unless Tep.gzip_cached("x" * 200) == ga
raise "digest hit: digest #{digests}, gzip #{gzips}" unless digests == 2 && gzips == 1

# A fresh String with the bytes of the last body: no digest, no gzip.
again = "x" * 200
raise "same object" if again.equal?(a)
raise "last-hit returned other bytes" unless Tep.gzip_cached(again) == ga
raise "last-hit ran the digest (#{digests})" unless digests == 2 && gzips == 1
last = Tep.instance_variable_get(:@gzip_last_raw)
raise "the last body is not a copy" if last != a || last.equal?(a)

# Same length, one byte different: a miss, not the last gzip, and the
# last body stays.
b = "x" * 199 + "y"
gb = Tep.gzip_cached(b)
same_body!(gb, b, "same-length miss")
raise "same-length body reused the last gzip" if gb == ga
raise "same-length miss: digest #{digests}, gzip #{gzips}" unless digests == 3 && gzips == 2
raise "a miss replaced the last body" unless Tep.gzip_cached("x" * 200) == ga && digests == 3

# The last body is a snapshot: a caller that mutates its String misses.
m = "m" * 200
Tep.gzip_cached(m)
Tep.gzip_cached(m)
m.replace("n" * 200)
same_body!(Tep.gzip_cached(m), "n" * 200, "mutated source")

# The digest table empties at GZIP_CACHE_MAX; the last body is not in it.
same_body!(Tep.gzip_cached("x" * 200), a, "x again")
Tep.gzip_cached("x" * 200)
bodies = (0..Tep::GZIP_CACHE_MAX).map { |i| "body #{i} " * 20 }
bodies.each { |s| Tep.gzip_cached(s) }
d = digests
same_body!(Tep.gzip_cached("x" * 200), a, "last after wipe")
raise "last after wipe ran the digest" unless digests == d
same_body!(Tep.gzip_cached(bodies.first.dup), bodies.first, "first after wipe")

# Threads alternating bodies of one length: every answer inflates to its own body.
pool = ["p" * 300, "q" * 300, "p" * 299 + "q"]
threads = 4.times.map do |t|
  Thread.new do
    300.times do |k|
      s = pool[(k + t) % pool.length]
      same_body!(Tep.gzip_cached(s.dup), s, "thread")
    end
  end
end
threads.each(&:join)
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
        "tep gzip last-hit failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn join_body_does_not_copy_a_one_part_rack_body() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"
part = "x" * 128
out = GzipCache.join_body([part])
raise "copied" unless out.equal?(part)
raise "empty" unless GzipCache.join_body([]) == ""
raise "joined" unless GzipCache.join_body(["a", "b"]) == "ab"
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
        "join_body failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn identical_fresh_strings_gzip_once_via_last_hit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end

a = "x" * 128
b = "x" * 128
raise "same object" if a.equal?(b)
app = lambda { |_env| [200, { "content-type" => "text/html" }, [a]] }
# Second call uses a different String of the same bytes — wrk's shape.
n_at = 0
wrapped = GzipCache.wrap(lambda { |_env|
  body = n_at == 0 ? a : b
  n_at += 1
  [200, { "content-type" => "text/html" }, [body]]
})
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
wrapped.call(env)
wrapped.call(env)
raise "gzipped #{n} times" unless n == 1
# Keys are [String#hash, bytesize, CRC-32] — small, never the body itself.
keys = GzipCache.instance_variable_get(:@store).keys
raise "key holds a body #{keys.inspect}" unless keys.all? { |k| k.is_a?(Array) && k.length == 3 && k.all?(Integer) }
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
        "last-hit gzip failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn last_hit_does_not_follow_a_mutated_source_string() {
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
wrapped = GzipCache.wrap(lambda { |_env|
  [200, { "content-type" => "text/html" }, [body]]
})
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
first = wrapped.call(env)
body.replace("y" * 128)
second = wrapped.call(env)
raise "gzipped #{n} times" unless n == 2
raise "mutated source reused gzip" if first[2][0] == second[2][0]
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
        "last-hit snapshot failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn digest_fallback_does_not_reuse_gzip_across_distinct_bodies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require "zlib"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

# Equal size, different bytes — would collide under CRC32+size alone if
# Zlib.crc32 happened to match; digest keys refuse wrong-body hits either way.
a_body = "a" * 256
b_body = "b" * 256
n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
ga = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [a_body]] }).call(env)
# Clear last-hit so the second request must use the Hash fallback.
GzipCache.instance_variable_set(:@last_raw, nil)
GzipCache.instance_variable_set(:@last_gz, nil)
gb = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [b_body]] }).call(env)
raise "same gzip across distinct bodies" if ga[2][0] == gb[2][0]
raise "gzipped #{n} times" unless n == 2
# Same body again via fallback (last-hit still cleared) must hit the digest store.
GzipCache.instance_variable_set(:@last_raw, nil)
GzipCache.instance_variable_set(:@last_gz, nil)
ga2 = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [a_body.dup]] }).call(env)
raise "digest miss" unless ga2[2][0] == ga[2][0]
raise "gzipped again #{n}" unless n == 2
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
        "digest fallback failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

/// The splice: a page that varies per request (a fresh token in the
/// layout) around a large cached fragment inflates to exactly its body,
/// with the fragment deflated once across requests. Non-ASCII text, a
/// repeated page reusing the last splice, a nested fragment, and a
/// fragment the body doesn't contain (the whole-body path) are each
/// covered. Compared as bytes: `Zlib.gunzip` returns BINARY.
#[test]
fn spliced_gzip_inflates_to_the_body_and_deflates_a_fragment_once() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
module Rails
  class MemoryStore
    def initialize; @d = {}; end
    def read_str(k); @d[k]; end
    def write_str(k, v, _ttl); @d[k] = v.dup.freeze; end
  end
end
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"
raise "splice unavailable" unless GzipCache::SPLICE_OK

store = Rails::MemoryStore.new
msgs = (1..400).map { |i| "<div class=\"message\" id=\"m#{i}\"><p>Message #{i} — héllo #{i * 7}</p></div>\n" }.join
store.write_str("coll", msgs, 0)

fragment_deflates = 0
orig = GzipCache.method(:raw_deflate)
GzipCache.define_singleton_method(:raw_deflate) do |d, dict, lv|
  fragment_deflates += 1 if lv == Zlib::DEFAULT_COMPRESSION
  orig.call(d, dict, lv)
end

bodies = []
seq = 0
app = GzipCache.wrap(lambda { |e|
  tok = e["TOK"] || (seq += 1).to_s * 16
  body = +"<html><head><meta content=#{tok}></head><body>" << store.read_str("coll") << "<form><input value=#{tok}></form></body></html>"
  bodies << body.dup
  [200, { "content-type" => "text/html" }, [body]]
})
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
3.times do |i|
  _, h, b = app.call(env)
  raise "encoding #{i}" unless h["content-encoding"] == "gzip"
  raise "length #{i}" unless h["content-length"] == b[0].bytesize.to_s
  raise "round trip #{i}" unless Zlib.gunzip(b[0]) == bodies[i].b
end
raise "fragment deflated #{fragment_deflates}x" unless fragment_deflates == 1
whole = Zlib.gzip(bodies.last).bytesize
_, _, last = app.call(env)
raise "spliced #{last[0].bytesize} B vs whole #{whole} B" unless last[0].bytesize < whole * 1.10

# The same page again (no per-request token) reuses the last splice.
GzipCache.instance_variable_set(:@last_raw, nil)
_, _, r1 = app.call(env.merge("TOK" => "same"))
GzipCache.instance_variable_set(:@last_raw, nil)
_, _, r2 = app.call(env.merge("TOK" => "same"))
raise "repeat not reused" unless r1[0].equal?(r2[0])
raise "repeat round trip" unless Zlib.gunzip(r2[0]) == bodies.last.b

# A fragment nested in a later one (a collection miss writes its members
# first) is found in order; the container is skipped.
member = store.write_str("m1", msgs[0, 6000], 0)
body = "pre-" + msgs + "-post"
raise "nested" unless Zlib.gunzip(GzipCache.splice(body, [member, store.read_str("coll")])) == body.b

# Not in the body: no splice, the whole-body path answers.
raise "spliced a stranger" unless GzipCache.splice("plain page " * 50, [store.read_str("coll")]).nil?
stray = GzipCache.wrap(lambda { |_e|
  GzipCache.note_fragment(store.read_str("coll"))
  [200, { "content-type" => "text/html" }, ["no fragment here " * 50]]
})
_, _, sb = stray.call(env)
raise "fallback round trip" unless Zlib.gunzip(sb[0]) == ("no fragment here " * 50).b
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
        "spliced gzip failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

/// The layout text around per-request CSRF tokens: each token the request
/// mints is cut out (a stored block) and the constant runs between tokens
/// are deflated once, so after the first request a page whose only
/// difference is its tokens deflates nothing. Covered: a token used twice,
/// a page with tokens and no fragment, and a minted token that is not in
/// the body (the whole-body path, unchanged).
#[test]
fn token_cut_text_runs_deflate_once_across_requests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
module Rails
  class MemoryStore
    def initialize; @d = {}; end
    def read_str(k); @d[k]; end
    def write_str(k, v, _ttl); @d[k] = v.dup.freeze; end
  end
end
require "securerandom"
require "base64"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"
raise "splice unavailable" unless GzipCache::SPLICE_OK

store = Rails::MemoryStore.new
store.write_str("coll", (1..300).map { |i| "<div id=\"m#{i}\">Message #{i} — héllo</div>\n" }.join, 0)
head = (1..80).map { |i| "<link rel=\"stylesheet\" href=\"/assets/s#{i}.css\">\n" }.join
nav = (1..60).map { |i| "<a href=\"/rooms/#{i}\">Room #{i}</a>\n" }.join

deflates = 0
orig = GzipCache.method(:raw_deflate)
GzipCache.define_singleton_method(:raw_deflate) { |d, dict, lv| deflates += 1; orig.call(d, dict, lv) }

bodies = []
page = lambda do |frag|
  GzipCache.wrap(lambda { |_e|
    t1 = Base64.urlsafe_encode64(SecureRandom.random_bytes(64), padding: false)
    t2 = Base64.urlsafe_encode64(SecureRandom.random_bytes(64), padding: false)
    GzipCache.note_token(t1)
    GzipCache.note_token(t2)
    body = +"<html><head><meta name=\"csrf-token\" content=\"#{t1}\">" << head << "</head><body>" << nav
    body << store.read_str("coll") if frag
    body << "<form><input name=\"authenticity_token\" value=\"#{t2}\"></form>" << nav << "<p data-t=\"#{t1}\">x</p></body></html>"
    bodies << body.dup
    [200, { "content-type" => "text/html" }, [body]]
  })
end
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }

[true, false].each do |frag|
  app = page.call(frag)
  _, _, b = app.call(env)
  raise "round trip first frag=#{frag}" unless Zlib.gunzip(b[0]) == bodies.last.b
  before = deflates
  3.times do |i|
    _, h, b = app.call(env)
    raise "length #{i}" unless h["content-length"] == b[0].bytesize.to_s
    raise "round trip #{i} frag=#{frag}" unless Zlib.gunzip(b[0]) == bodies.last.b
  end
  raise "deflated #{deflates - before}x after the first request (frag=#{frag})" unless deflates == before
  whole = Zlib.gzip(bodies.last).bytesize
  raise "spliced #{b[0].bytesize} B vs whole #{whole} B" unless b[0].bytesize < whole * 1.25
end

# A minted token that is not in the body, and no fragment: the whole-body path.
stray = GzipCache.wrap(lambda { |_e|
  GzipCache.note_token("not-in-the-body-token")
  [200, { "content-type" => "text/html" }, ["plain page " * 200]]
})
_, _, sb = stray.call(env)
raise "fallback round trip" unless Zlib.gunzip(sb[0]) == ("plain page " * 200).b
raise "spliced a token-free body" unless GzipCache.splice("plain page " * 200, [], ["not-in-the-body-token"]).nil?

# Randomized: runs from a small vocabulary recur after different
# predecessors and gaps (tokens of varying length, short stored runs
# between them, now and then a NUL), so cached pieces are reused under
# every combination; each body must inflate to itself.
rng = Random.new(42)
vocab = (0...6).map { |k| (1..(5 + k * 40)).map { |i| "<li class=\"v#{k}\">item #{i} of #{k}</li>" }.join }
vocab << "short"
vocab << ("<p>nul\0inside</p>" * 30)
400.times do |it|
  toks = []
  body = +""
  (2 + rng.rand(6)).times do
    body << vocab[rng.rand(vocab.length)]
    tok = Base64.urlsafe_encode64(SecureRandom.random_bytes(8 + rng.rand(70)), padding: false)
    toks << tok
    body << tok
  end
  body << vocab[rng.rand(vocab.length)]
  gz = GzipCache.splice(body, [], toks)
  next if gz.nil?
  raise "random round trip #{it}" unless Zlib.gunzip(gz) == body.b
end
puts "ALL OK"
"##;
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
        "token-cut splice failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

/// Predecessor-fragment dictionaries (ports' page_parts / deflater):
/// consecutive message fragments with short glue deflate against the run's
/// preceding window, stay correct when CSRF in the layout varies, refuse a
/// piece after the wrong predecessor, and beat independent (no-dict) pieces.
#[test]
fn predecessor_fragment_dictionary_reuses_pieces_safely() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require "securerandom"
require "base64"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"
raise "splice unavailable" unless GzipCache::SPLICE_OK
raise "raw dictionaries required for this gate" unless GzipCache::RAW_DICTIONARY_OK

tmpl = lambda { |i|
  b = +"<turbo-frame id=\"message_#{i}\" class=\"message\">"
  50.times { |p|
    b << "<p class=\"message__body\">Paragraph #{p} of message #{i}: shared campfire markup "
    b << "<a href=\"/rooms/1\">room</a> repeated for dictionary matches.</p>\n"
  }
  (b << "</turbo-frame>\n").freeze
}
msgs = (1..12).map { |i| tmpl.call(i) }
# Short glue between messages (≤ MAX_GLUE) so they form one run.
inner = msgs.join("\n")

fragment_deflates = 0
dicts = 0
orig = GzipCache.method(:raw_deflate)
GzipCache.define_singleton_method(:raw_deflate) do |d, dict, lv|
  # Layout constant_run also uses DEFAULT_COMPRESSION; count only message
  # pieces (glue may precede the turbo-frame).
  if lv == Zlib::DEFAULT_COMPRESSION && d.include?("turbo-frame")
    fragment_deflates += 1
    dicts += 1 unless dict.nil?
  end
  orig.call(d, dict, lv)
end

bodies = []
2.times do
  tok = Base64.urlsafe_encode64(SecureRandom.random_bytes(64), padding: false)
  body = +"<html><head><meta name=\"csrf-token\" content=\"#{tok}\"></head><body>"
  body << ("<nav>Room link #{'x' * 40}</nav>\n" * 20)
  body << inner
  body << "<form><input name=\"authenticity_token\" value=\"#{tok}\"></form></body></html>"
  bodies << body
  gz = GzipCache.splice(body, msgs, [tok])
  raise "nil splice" if gz.nil?
  raise "round trip" unless Zlib.gunzip(gz) == body.b
  # RFC 1952 gzip member: ID1/ID2/CM and a valid inflate.
  raise "bad magic" unless gz.byteslice(0, 3) == "\x1f\x8b\x08".b
end
raise "fragment deflated #{fragment_deflates}x (want #{msgs.size})" unless fragment_deflates == msgs.size
raise "no dictionaries used (#{dicts})" unless dicts == msgs.size - 1

# Same fragments, different predecessor order: must not reuse the A→B piece
# after B→A (would be a wrong-body inflate if the chain key were ignored).
a, b = msgs[0], msgs[1]
ab = a + "\n" + b
ba = b + "\n" + a
gz_ab = GzipCache.splice(ab, [a, b])
gz_ba = GzipCache.splice(ba, [b, a])
raise "ab round trip" unless Zlib.gunzip(gz_ab) == ab.b
raise "ba round trip" unless Zlib.gunzip(gz_ba) == ba.b
raise "order collision" if gz_ab == gz_ba

# Microbench: predecessor dicts beat independent no-dict pieces on the same
# glue-folded run (the ports' ~4× story on message lists; here a floor).
no_dict = GzipCache::GZIP_HEADER.bytesize + GzipCache::FINAL_BLOCK.bytesize + 8
msgs.each_with_index do |m, i|
  data = i.zero? ? m : ("\n".b + m)
  no_dict += GzipCache.raw_deflate(data, nil, Zlib::DEFAULT_COMPRESSION).bytesize
end
with_dict = GzipCache.splice(inner, msgs).bytesize
raise "dict #{with_dict} B not below no-dict #{no_dict} B" unless with_dict < no_dict
ratio = no_dict.to_f / with_dict
raise "weak dict gain #{ratio}" unless ratio >= 1.25
puts "MICROBENCH dict=#{with_dict} no_dict=#{no_dict} gain=#{ratio.round(2)}x"
puts "ALL OK"
"##;
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
        "predecessor-dict splice failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
    eprintln!("{stdout}");
}

/// Short glue that carries a recorded CSRF token must not fold into the
/// next fragment piece (CodeRabbit on #546): each token change would miss
/// the piece cache and re-deflate glue+fragment. Token-bearing glue stays
/// on splice_text; fragments deflate once across requests.
#[test]
fn token_bearing_short_glue_does_not_fold_into_fragment_piece() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require "securerandom"
require "base64"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"
raise "splice unavailable" unless GzipCache::SPLICE_OK

tmpl = lambda { |i|
  b = +"<turbo-frame id=\"message_#{i}\" class=\"message\">"
  40.times { |p|
    b << "<p class=\"message__body\">Paragraph #{p} of message #{i}: shared markup.</p>\n"
  }
  (b << "</turbo-frame>\n").freeze
}
a = tmpl.call(1)
b = tmpl.call(2)
raise "glue budget" unless a.bytesize >= GzipCache::SPLICE_MIN

fragment_deflates = 0
orig = GzipCache.method(:raw_deflate)
GzipCache.define_singleton_method(:raw_deflate) do |d, dict, lv|
  if lv == Zlib::DEFAULT_COMPRESSION && d.include?("turbo-frame")
    fragment_deflates += 1
  end
  orig.call(d, dict, lv)
end

4.times do
  tok = Base64.urlsafe_encode64(SecureRandom.random_bytes(32), padding: false)
  raise "token longer than MAX_GLUE" unless tok.bytesize <= GzipCache::MAX_GLUE
  # Token alone is the entire short glue between two fragments.
  body = a + tok + b
  gz = GzipCache.splice(body, [a, b], [tok])
  raise "nil splice" if gz.nil?
  raise "round trip" unless Zlib.gunzip(gz) == body.b
end
raise "fragment deflated #{fragment_deflates}x (want 2 — once each, not per token)" unless fragment_deflates == 2

# Control: token-free short glue still folds. Fold is the second piece
# being glue+frag (`\n` + turbo-frame). A dictionary is only required
# when raw dictionaries work; JRuby / #526 passes nil and still folds.
folded = 0
dicts = 0
GzipCache.define_singleton_method(:raw_deflate) do |d, dict, lv|
  if lv == Zlib::DEFAULT_COMPRESSION && d.start_with?("\n") && d.include?("turbo-frame")
    folded += 1
    dicts += 1 unless dict.nil?
  end
  orig.call(d, dict, lv)
end
c = tmpl.call(3)
d = tmpl.call(4)
gz = GzipCache.splice(c + "\n" + d, [c, d])
raise "fold rt" unless Zlib.gunzip(gz) == (c + "\n" + d).b
raise "token-free glue did not fold (folded=#{folded})" unless folded >= 1
if GzipCache::RAW_DICTIONARY_OK
  raise "expected dict on fold (dicts=#{dicts})" unless dicts >= 1
end
puts "ALL OK"
"##;
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
        "token-bearing glue fold failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());

    // Same fold control with raw dictionaries refused (#526 / JRuby).
    let fallback = r#"
require "zlib"
Zlib::Deflate.prepend(Module.new do
  def set_dictionary(_dictionary)
    raise Zlib::StreamError, "raw dictionaries unsupported"
  end
end)
require File.expand_path("runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache", Dir.pwd)
raise "expected no raw dict" if GzipCache::RAW_DICTIONARY_OK
c = ("<turbo-frame id=\"c\">" + ("<p>x</p>\n" * 80) + "</turbo-frame>\n").freeze
d = ("<turbo-frame id=\"d\">" + ("<p>y</p>\n" * 80) + "</turbo-frame>\n").freeze
folded = 0
dicts = 0
orig = GzipCache.method(:raw_deflate)
GzipCache.define_singleton_method(:raw_deflate) do |data, dict, lv|
  if lv == Zlib::DEFAULT_COMPRESSION && data.start_with?("\n") && data.include?("turbo-frame")
    folded += 1
    dicts += 1 unless dict.nil?
  end
  orig.call(data, dict, lv)
end
body = c + "\n" + d
raise "fold rt" unless Zlib.gunzip(GzipCache.splice(body, [c, d])) == body.b
raise "fallback did not fold (folded=#{folded})" unless folded >= 1
raise "fallback passed a dict (dicts=#{dicts})" unless dicts.zero?
puts "ALL OK"
"#;
    let fb = Command::new("ruby")
        .arg("-e")
        .arg(fallback)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let fb_out = String::from_utf8_lossy(&fb.stdout);
    let fb_err = String::from_utf8_lossy(&fb.stderr);
    assert!(
        fb_out.contains("ALL OK"),
        "no-dict fold control failed\n=== stdout ===\n{fb_out}\n=== stderr ===\n{fb_err}"
    );
    assert!(fb.status.success(), "fallback driver exited {:?}", fb.status.code());
}

/// The same frozen fragment twice in one run (self-predecessor) must
/// still inflate, and the WeakKeyMap value must not strongly retain the
/// key (CodeRabbit outside-diff on #546).
#[test]
fn repeated_fragment_chain_does_not_pin_weak_map_key() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"
raise "splice unavailable" unless GzipCache::SPLICE_OK
frag = ("<turbo-frame id=\"dup\">" + ("<p>same row markup</p>\n" * 80) + "</turbo-frame>\n").freeze
body = frag + "\n" + frag
gz = GzipCache.splice(body, [frag, frag])
raise "nil" if gz.nil?
raise "round trip" unless Zlib.gunzip(gz) == body.b
list = GzipCache.instance_variable_get(:@pieces)[frag]
raise "no piece" if list.nil? || list.empty?
list.each do |e|
  chain = e[0]
  i = 0
  while i < chain.length
    pred = chain[i]
    raise "strong pred #{pred.class}" unless pred.is_a?(WeakRef)
    raise "dead self-pred" unless pred.weakref_alive? && pred.__getobj__.equal?(frag)
    i += 2
  end
end
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
        "weak chain pin failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
