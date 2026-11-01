//! A mounted Rack app sees Rails' shifted environment and owns its response.

use super::emit_and_run::{self, Overlay};
use std::process::Command;

const APP: &str = r#"class StaticFiles
  def self.call(env)
    [207, {"content-type" => "text/plain", "x-script" => env["SCRIPT_NAME"], "x-path" => env["PATH_INFO"]}, ["first:", env["QUERY_STRING"].to_s, ":last"]]
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  get "/files/before", to: "home#index"
  mount StaticFiles, at: "/files"
  get "/files/after", to: "home#index"
  match "/files/head", to: "home#index", via: :head
end
"#;

fn app(source: &str, routes: &str) -> Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n")
        .write("app/controllers/static_files.rb", source)
        .write("app/controllers/home_controller.rb", "class HomeController < ActionController::Base\n  def index\n    render plain: 'controller'\n  end\nend\n")
        .write("config/routes.rb", routes)
}

const ORACLE_GUARD: &str = r#"
begin
  unless RailsOracle.available?("railties", "actionpack", "actionview")
    warn "skipping Rack mount oracle: #{RailsOracle.label} requires railties, actionpack and actionview"
    exit 77
  end
  RailsOracle.activate("railties", "actionpack", "actionview")
  require "rails"
  require "action_controller"
rescue LoadError => error
  warn "skipping Rack mount oracle: #{RailsOracle.label} dependency unavailable: #{error.message}"
  exit 77
end
"#;

fn oracle(source: &str, routes: &str, probe: &str) -> Option<serde_json::Value> {
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/support/rails_oracle.rb");
    let native = format!(r#"
require {helper:?}
{ORACLE_GUARD}
{source}
class HomeController < ActionController::Base
  def index
    render plain: "controller"
  end
end
endpoint = ActionDispatch::Routing::RouteSet.new
endpoint.draw do
{routes}
end
{probe}
"#);
    let output = Command::new("ruby").args(["-e", &native]).output().expect("Rails oracle");
    if output.status.code() == Some(77) {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return None;
    }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    Some(serde_json::from_slice(&output.stdout).expect("Rails oracle JSON"))
}

/// The review's exact expanded matrix: eight verbs by twenty paths. These
/// always run on emitted output; the Rails comparison needs optional gems.
#[test]
fn rack_mount_environment_and_response_match_rails() {
    let probe = r#"
require "json"
require "stringio"
rows = []
["GET", "POST", "HEAD", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE"].each do |method|
  ["/files", "/files/", "/files/a/b.txt", "/files/a/", "/files//a", "//files///a/", "/files/a%2fb", "/files.json", "/files-other", "/filesystem", "/files/before", "/files/after", "/files/head", "/elsewhere", "/files/../outside", "/Files", "/f%6f/a", "/f%6F/a", "/files%20x", "/files/%c3%a9"].each do |path|
    env = {"REQUEST_METHOD" => method, "SCRIPT_NAME" => "/outer", "PATH_INFO" => path,
      "QUERY_STRING" => "q=one%20two&x=%2f", "rack.input" => StringIO.new("payload"),
      "rack.url_scheme" => "http", "HTTP_HOST" => "example.org", "SERVER_NAME" => "example.org",
      "SERVER_PORT" => "80", "SERVER_PROTOCOL" => "HTTP/1.1", "rack.errors" => $stderr}
    status, headers, body = endpoint.call(env)
    chunks = []
    body.each { |chunk| chunks << chunk }
    text = status == 404 && !headers.key?("x-path") ? nil : chunks.join
    rows << [method, path, status, headers["x-script"], headers["x-path"], text]
    body.close if body.respond_to?(:close)
  end
end
puts JSON.generate(rows)
"#;
    let routes = ROUTES.strip_prefix("Rails.application.routes.draw do\n").unwrap().strip_suffix("end\n").unwrap();
    let want = oracle(APP, routes, probe);
    for source in [
        APP.to_string(),
        APP.replace("class StaticFiles", "module StaticFiles"),
        APP.replace("  def self.call(env)\n", "  class << self\n  def call(env)\n")
            .replace("\nend\n", "\n  end\nend\n"),
    ] {
        let run = app(&source, ROUTES).run_ruby(&format!("endpoint = Main.method(:run_rack)\n{probe}"));
        run.assert_passes();
        let got: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap();
        let rows = got.as_array().unwrap();
        assert_eq!(rows.len(), 160);
        for row in rows.iter().filter(|row| row[1] == "/files") {
            assert_eq!(row[2], 207);
            assert_eq!(row[3], "/outer/files");
            assert_eq!(row[4], "/");
            assert_eq!(row[5], "first:q=one%20two&x=%2f:last");
        }
        if let Some(want) = &want { assert_eq!(&got, want); }
    }
}

/// A nested target can share its enclosing class's emitted file. Boot must
/// load that file, while module-only namespaces retain the target's own file.
#[test]
fn rack_mount_nested_targets_load_the_emitted_file_anchor_and_match_rails() {
    use roundhouse::project::BuildTarget;
    let probe = r#"
require "json"
require "stringio"
rows = []
["GET", "POST"].each do |method|
  ["/files", "/files/child.json"].each do |path|
    env = {"REQUEST_METHOD" => method, "SCRIPT_NAME" => "/outer", "PATH_INFO" => path,
      "QUERY_STRING" => "q=1", "rack.input" => StringIO.new("payload"),
      "rack.url_scheme" => "http", "HTTP_HOST" => "example.org"}
    status, headers, body = endpoint.call(env)
    chunks = []
    body.each { |chunk| chunks << chunk }
    rows << [method, path, status, headers, chunks.join]
    body.close if body.respond_to?(:close)
  end
end
puts JSON.generate(rows)
"#;
    for (source, target, anchor, absent) in [
        (format!("class RackHost\n  class V1\n{APP}  end\nend\n"),
            "RackHost::V1::StaticFiles", "models/rack_host", Some("models/rack_host/v1/static_files.rb")),
        (format!("module RackNamespace\n  class Host\n{APP}  end\nend\n"),
            "RackNamespace::Host::StaticFiles", "models/rack_namespace/host", Some("models/rack_namespace/host/static_files.rb")),
        (format!("module RackNamespace\n{APP}end\n"),
            "RackNamespace::StaticFiles", "models/rack_namespace/static_files", None),
        (APP.to_string(), "StaticFiles", "models/static_files", None),
    ] {
        let route = format!("mount {target}, at: '/files'");
        let routes = format!("Rails.application.routes.draw do\n{route}\nend\n");
        let want = oracle(&source, &route, probe);
        let run = app(&source, &routes).run_ruby(&format!("endpoint = Main.method(:run_rack)\n{probe}"));
        run.assert_passes();
        let got: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap();
        assert_eq!(got.as_array().unwrap().len(), 4);
        assert!(got.as_array().unwrap().iter().all(|row| row[2] == 207 && row[4] == "first:q=1:last"), "{got}");
        if let Some(want) = &want { assert_eq!(&got, want, "{target}"); }
        for build in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
            let (tree, errors) = app(&source, &routes).emit(build);
            assert!(errors.is_empty(), "{build:?}: {errors:?}");
            let adapter = std::fs::read_to_string(tree.join("config/rack_mounts.rb")).unwrap();
            assert!(adapter.contains(&format!("require_relative \"../app/{anchor}\"")), "{build:?}: {adapter}");
            assert!(tree.join(format!("app/{anchor}.rb")).is_file(), "{build:?}: {anchor}");
            if let Some(absent) = absent {
                assert!(!tree.join(format!("app/{absent}")).exists(), "{build:?}: unexpected standalone target");
            }
        }
    }
}

#[test]
fn rack_mount_literal_boundary_leaves_later_routes_reachable() {
    let routes = r#"mount StaticFiles, at: "/files"
get "/filesystem", to: "home#index"
get "/files0", to: "home#index"
get "/files_", to: "home#index""#;
    let probe = r#"
require "json"
require "stringio"
rows = ["/filesystem", "/files0", "/files_", "/files.json", "/files-other"].map do |path|
  status, headers, body = endpoint.call({"REQUEST_METHOD" => "GET", "SCRIPT_NAME" => "/outer", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new, "rack.url_scheme" => "http", "HTTP_HOST" => "example.org"})
  chunks = []
  body.each { |chunk| chunks << chunk }
  text = chunks.join
  body.close if body.respond_to?(:close)
  [path, status, headers["x-script"], headers["x-path"], text]
end
puts JSON.generate(rows)
"#;
    let want = oracle(APP, routes, probe);
    let run = app(APP, &format!("Rails.application.routes.draw do\n{routes}\nend\n"))
        .run_ruby(&format!("endpoint = Main.method(:run_rack)\n{probe}"));
    run.assert_passes();
    let got: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap();
    assert_eq!(got, serde_json::json!([
        ["/filesystem", 200, null, null, "controller"],
        ["/files0", 200, null, null, "controller"],
        ["/files_", 200, null, null, "controller"],
        ["/files.json", 207, "/outer/files", "/.json", "first::last"],
        ["/files-other", 207, "/outer/files", "/-other", "first::last"],
    ]));
    if let Some(want) = &want { assert_eq!(&got, want); }
}

#[test]
fn rack_mount_oracle_missing_gems_and_dependencies_skip_in_ci_and_locally() {
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let empty = std::env::temp_dir().join(format!("rack-oracle-no-gems-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let partial = empty.join("partial");
    std::fs::create_dir_all(partial.join("specifications")).unwrap();
    for name in ["railties", "actionpack", "actionview"] {
        std::fs::write(partial.join(format!("specifications/{name}-8.1.4.gemspec")), format!(
            "Gem::Specification.new do |s|\ns.name = '{name}'\ns.version = '8.1.4'\ns.add_runtime_dependency 'roundhouse_missing_oracle_dependency', '>= 0'\nend\n"
        )).unwrap();
    }
    for gems in [&empty, &partial] {
        for ci in [true, false] {
            let mut command = Command::new("ruby");
            command.args(["-e", &format!("require {helper:?}\n{ORACLE_GUARD}")])
                .env("GEM_HOME", gems).env("GEM_PATH", gems)
                .env("RH_RAILS_ORACLE", "8.1.4").env_remove("RUBYOPT");
            if ci { command.env("CI", "true"); } else { command.env_remove("CI"); }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(77), "{}", String::from_utf8_lossy(&output.stderr));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("skipping Rack mount oracle"), "{stderr}");
            if gems == &partial { assert!(stderr.contains("roundhouse_missing_oracle_dependency"), "{stderr}"); }
        }
    }
    std::fs::remove_dir_all(empty).unwrap();
}

/// No response reconstruction: preserve status, header arrays, the triple and
/// a streaming body, including its close ownership. Other env entries survive.
#[test]
fn rack_mount_passes_the_original_env_and_response_objects() {
    app(APP, ROUTES).run_ruby(r#"
require "stringio"
body = Object.new
def body.each
  raise "dispatcher enumerated the Rack caller's body"
end
def body.close
  raise "dispatcher closed the Rack caller's body"
end
headers = {"set-cookie" => ["a=1", "b=2"], "x-custom" => "unchanged"}
triple = [209, headers, body]
input = StringIO.new("untouched input")
marker = Object.new
env = {"REQUEST_METHOD" => "POST", "SCRIPT_NAME" => "/parent", "PATH_INFO" => "/files/child.json", "rack.input" => input, "app.marker" => marker}
StaticFiles.define_singleton_method(:call) do |seen|
  raise "copied env" unless seen.equal?(env)
  raise "lost env" unless seen["rack.input"].equal?(input) && seen["app.marker"].equal?(marker)
  raise "read input" unless input.pos == 0
  raise "script name" unless seen["SCRIPT_NAME"] == "/parent/files"
  raise "path info" unless seen["PATH_INFO"] == "/child.json"
  triple
end
raise "rebuilt triple" unless Main.run_rack(env).equal?(triple)
raise "changed status or headers" unless triple[0] == 209 && headers == {"set-cookie" => ["a=1", "b=2"], "x-custom" => "unchanged"}
"#).assert_passes();
}

#[test]
fn rack_mount_cgi_serializes_the_supplied_response() {
    app(APP, ROUTES).run_ruby(r#"
require "stringio"
env = {"REQUEST_METHOD" => "GET", "SCRIPT_NAME" => "/outer", "PATH_INFO" => "/files/child", "QUERY_STRING" => "q=one"}
output = StringIO.new
Main.run(env, StringIO.new("input"), output)
wire = output.string
raise wire unless wire.start_with?("Status: 207\r\n")
raise wire unless wire.include?("x-script: /outer/files\r\n") && wire.include?("x-path: /child\r\n")
raise wire unless wire.end_with?("\r\n\r\nfirst:q=one:last")
"#).assert_passes();
}

#[test]
fn unproven_rack_mount_has_a_rack_specific_located_error() {
    for source in [
        "class StaticFiles; end",
        "class StaticFiles; def call(env); [200, {}, []]; end; end",
        "class StaticFiles; def self.call; [200, {}, []]; end; end",
    ] {
        let errors = app(
            source,
            "Rails.application.routes.draw do\n  mount StaticFiles, at: \"/files\"\nend\n",
        )
        .check_errors();
        assert!(errors.iter().any(|error| error.contains("Rack target `StaticFiles` is not a unique in-app class or module with a source-defined public singleton `call(env)` taking one positional argument")), "{errors:?}");
        assert!(
            !errors
                .iter()
                .any(|error| error.contains("PATH Rails engine")),
            "{errors:?}"
        );
    }
}

#[test]
fn rack_mount_scope_and_duplicate_names_remain_explicit_errors() {
    let scoped = app(APP, "Rails.application.routes.draw do\n  scope '/admin' do\n    mount StaticFiles, at: '/files'\n  end\nend\n").check_errors();
    assert!(
        scoped
            .iter()
            .any(|error| error.contains("only top-level literal in-app Rack mounts are composed")),
        "{scoped:?}"
    );
    let repeated = app(APP, "Rails.application.routes.draw do\n  mount StaticFiles, at: '/files'\n  mount StaticFiles, at: '/other'\nend\n").check_errors();
    assert!(
        repeated.iter().any(|error| error.contains(
            "mounting the same Rack app constant more than once requires distinct `as:` names"
        )),
        "{repeated:?}"
    );
}


#[test]
fn rack_mount_declines_to_intervening_controllers_and_later_mounts_like_rails() {
    let source = format!("{}\n{}", APP.replace("207, {", "404, {\"x-cascade\" => \"pass\", "), APP.replace("StaticFiles", "NextFiles").replace("207,", "208,"));
    let routes = r#"mount StaticFiles, at: "/files"
get "/files/controller", to: "home#index"
mount NextFiles, at: "/files"
get "/files/later", to: "home#index""#;
    let probe = r#"
require "json"
require "stringio"
rows = ["/files/controller", "/files/later", "/files", "/files.json"].map do |path|
  env = {"REQUEST_METHOD" => "GET", "SCRIPT_NAME" => "/outer", "PATH_INFO" => path, "QUERY_STRING" => "kept=yes", "rack.input" => StringIO.new("unchanged"), "rack.url_scheme" => "http", "HTTP_HOST" => "example.org"}
  status, headers, body = endpoint.call(env)
  chunks = []
  body.each { |chunk| chunks << chunk }
  body.close if body.respond_to?(:close)
  [path, status, headers["x-script"], headers["x-path"], chunks.join]
end
puts JSON.generate(rows)
"#;
    let want = oracle(&source, routes, probe);
    let run = app(&source, &format!("Rails.application.routes.draw do\n{routes}\nend\n"))
        .run_ruby(&format!("endpoint = Main.method(:run_rack)\n{probe}"));
    run.assert_passes();
    let got: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap();
    assert_eq!(got, serde_json::json!([
        ["/files/controller", 200, null, null, "controller"],
        ["/files/later", 208, "/outer/files", "/later", "first:kept=yes:last"],
        ["/files", 208, "/outer/files", "/", "first:kept=yes:last"],
        ["/files.json", 208, "/outer/files", "/.json", "first:kept=yes:last"],
    ]));
    if let Some(want) = &want { assert_eq!(&got, want); }
}

#[test]
fn rack_mount_closes_discarded_bodies_and_restores_the_environment() {
    let source = format!("{APP}\n{}", APP.replace("StaticFiles", "NextFiles"));
    app(&source, "Rails.application.routes.draw do\nmount StaticFiles, at: '/files'\nmount NextFiles, at: '/files'\nend\n").run_ruby(r#"
body = Object.new
body.instance_variable_set(:@closes, 0)
def body.each; raise "enumerated a discarded body"; end
def body.close; @closes += 1; end
def body.closes; @closes; end
env = {"REQUEST_METHOD" => "GET", "SCRIPT_NAME" => "/outer", "PATH_INFO" => "/files/child", "QUERY_STRING" => "keep=yes"}
StaticFiles.define_singleton_method(:call) { |seen| [404, {"x-cascade" => "pass"}, body] }
final = [209, {"set-cookie" => ["a=1", "b=2"]}, Object.new]
NextFiles.define_singleton_method(:call) do |seen|
  raise "script shifted twice" unless seen["SCRIPT_NAME"] == "/outer/files"
  raise "path shifted twice" unless seen["PATH_INFO"] == "/child"
  raise "lost query" unless seen["QUERY_STRING"] == "keep=yes"
  final
end
raise "rebuilt final response" unless Main.run_rack(env).equal?(final)
raise "did not close declined body once" unless body.closes == 1
StaticFiles.define_singleton_method(:call) { |seen| [404, {"X-Cascade" => "pass"}, body] }
response = Main.run_rack({"REQUEST_METHOD" => "GET", "SCRIPT_NAME" => "/outer", "PATH_INFO" => "/files"})
raise "accepted uppercase cascade" unless response[1] == {"X-Cascade" => "pass"} && body.closes == 1
"#).assert_passes();
}

#[test]
fn rack_mount_cgi_closes_on_success_enumeration_failure_and_output_failure() {
    app(APP, ROUTES).run_ruby(r#"
require "stringio"
class OwnedRackBody
  attr_reader :closes, :reads
  def initialize(fail_each)
    @fail_each = fail_each
    @closes = @reads = 0
  end
  def each
    @reads += 1
    yield "chunk"
    raise IOError, "enumeration failed" if @fail_each
  end
  def close; @closes += 1; end
end
[[nil, false], [1, false], [2, false], [4, false], [nil, true]].each do |fail_write, fail_each|
  body = OwnedRackBody.new(fail_each)
  StaticFiles.define_singleton_method(:call) { |env| [200, {"content-type" => "text/plain"}, body] }
  output = StringIO.new
  writes = 0
  output.define_singleton_method(:write) do |value|
    writes += 1
    raise IOError, "output failed" if writes == fail_write
    super(value)
  end
  raised = false
  begin
    Main.run({"REQUEST_METHOD" => "GET", "SCRIPT_NAME" => "", "PATH_INFO" => "/files"}, StringIO.new, output)
  rescue IOError
    raised = true
  end
  raise "wrong error path" unless raised == (!!fail_write || fail_each)
  raise "body was not closed exactly once" unless body.closes == 1
end
"#).assert_passes();
}

/// Exercise the generated native conversion in the always-active unit lane,
/// including its exception paths. The native lane also drives a compiled HTTP
/// server; this unit test needs only the same gems as its emitted Ruby peers.
#[test]
fn rack_mount_native_conversion_serializes_headers_and_closes_consumed_bodies() {
    let (native, errors) = app(APP, ROUTES).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let main = std::fs::read_to_string(native.join("main.rb")).unwrap();
    let start = main.find("    rack_index = RouteTable.rack_index(").unwrap();
    let end = start + main[start..].find("    matched = ActionDispatch::Router.match(verb, request_path, Main.route_table)").unwrap();
    let fragment = &main[start..end];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let harness = root.join("tests/tep_server_harness.rb");
    let cache = root.join("runtime/spinel/tep/thruster_cache.rb");
    app(APP, ROUTES).run_ruby(&format!(r#"
require {harness:?}
require {cache:?}
module NativeRackProbe
  def self.dispatch(req, res)
    verb = req.verb
    request_path = req.path
{fragment}
  end
end
class NativeOwnedBody
  attr_reader :closes
  def initialize(fail_each); @fail_each = fail_each; @closes = 0; end
  def each
    yield "native-body"
    raise IOError, "enumeration failed" if @fail_each
  end
  def close; @closes += 1; end
end
class FailingNativeHeaders < Tep::Response
  def append_header(name, value); raise IOError, "response conversion failed"; end
end
["success", "enumeration", "headers"].each do |mode|
  body = NativeOwnedBody.new(mode == "enumeration")
  StaticFiles.define_singleton_method(:call) {{ |env| [200, {{"content-type" => "text/plain", "set-cookie" => ["a=1; Path=/", "b=2; Path=/"], "x-repeat" => ["first", "second"]}}, body] }}
  req = Tep::Request.new
  req.verb = "GET"
  req.path = "/files"
  res = mode == "headers" ? FailingNativeHeaders.new : Tep::Response.new
  raised = false
  begin
    NativeRackProbe.dispatch(req, res)
  rescue IOError
    raised = true
  end
  raise "wrong failure path" unless raised == (mode != "success")
  raise "unclosed body" unless body.closes == 1
  if mode == "success"
    wire = Tep.header_lines(res)
    raise wire unless wire.include?("Set-Cookie: a=1; Path=/\r\n") && wire.include?("Set-Cookie: b=2; Path=/\r\n")
    raise wire unless wire.lines.grep(/\Ax-repeat: /i).map {{ |line| line.split(": ", 2)[1].strip }} == ["first", "second"]
    raise res.body unless res.body == "native-body"
    res.headers["Cache-Control"] = "public, max-age=60"
    cache = Tep::ThrusterCache.new(100000)
    cache.store(req, res)
    copy = Tep::Response.new
    raise "cache missed" unless cache.answer(req, copy)
    cached_wire = Tep.header_lines(copy)
    raise cached_wire unless cached_wire.lines.grep(/\Ax-repeat: /i).map {{ |line| line.split(": ", 2)[1].strip }} == ["first", "second"]
  end
end
"#)).assert_passes();
}


/// The review's split fields must behave like their comma-joined equivalent,
/// through the emitted native adapter and the actual cache, not just on wire.
#[test]
fn rack_mount_native_cache_reads_every_policy_and_variant_field() {
    let (native, errors) = app(APP, ROUTES).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let main = std::fs::read_to_string(native.join("main.rb")).unwrap();
    let start = main.find("    rack_index = RouteTable.rack_index(").unwrap();
    let end = start + main[start..].find("    matched = ActionDispatch::Router.match(verb, request_path, Main.route_table)").unwrap();
    let fragment = &main[start..end];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let harness = root.join("tests/tep_server_harness.rb");
    let cache_source = root.join("runtime/spinel/tep/thruster_cache.rb");
    app(APP, ROUTES).run_ruby(&format!(r#"
require {harness:?}
require {cache_source:?}
module NativeRackProbe
  def self.dispatch(req, res)
    verb = req.verb
    request_path = req.path
{fragment}
  end
end

def cache_request(language, encoding = "gzip")
  req = Tep::Request.new
  req.verb = "GET"
  req.path = "/files"
  req.req_headers["host"] = "example.org"
  req.req_headers["accept-language"] = language
  req.req_headers["accept-encoding"] = encoding
  req
end

[
  ["no-cache", ["public, max-age=60", "no-cache"], nil, [false, false, false, false], ["en:1", "fr:2", "en:3", "fr:4"]],
  ["vary", "public, max-age=60", ["Accept-Encoding", "Accept-Language"], [false, false, true, true], ["en:1", "fr:2", "en:1", "fr:2"]],
  ["vary-star", "public, max-age=60", ["Accept-Encoding", "*"], [false, false, false, false], ["en:1", "fr:2", "en:3", "fr:4"]],
  ["policy", ["public", "max-age=60"], nil, [false, true, true, true], ["en:1", "en:1", "en:1", "en:1"]]
].each do |label, policy, vary, expected_hits, expected_bodies|
  observed = []
  [false, true].each do |combined|
    calls = 0
    headers = {{"content-type" => "text/plain", "cache-control" => (combined ? Array(policy).join(", ") : policy)}}
    headers["vary"] = combined ? Array(vary).join(", ") : vary if vary
    StaticFiles.define_singleton_method(:call) do |env|
      calls += 1
      [200, headers, [env["HTTP_ACCEPT_LANGUAGE"], ":", calls.to_s]]
    end
    cache = Tep::ThrusterCache.new(100000)
    cache.enabled = true
    hits = []
    bodies = []
    ["en", "fr", "en", "fr"].each do |language|
      req = cache_request(language)
      res = Tep::Response.new
      hit = cache.answer(req, res)
      unless hit
        NativeRackProbe.dispatch(req, res)
        cache.store(req, res)
      end
      hits << hit
      bodies << res.body
    end
    raise [label, hits, bodies].inspect unless hits == expected_hits && bodies == expected_bodies
    observed << [hits, bodies]
  end
  raise "split and combined differ" unless observed[0] == observed[1]
end

# Case-insensitive lookup must include values from both collections, in their
# serialization order, while leaving the raw fields available to the writer.
response = Tep::Response.new
response.append_header("Cache-Control", "public, max-age=60")
response.append_header("cache-control", "no-cache")
raise "case-sensitive policy" unless Tep::ThrusterCache.lifetime(response) == 0
raise "lost field order" unless Tep::ThrusterCache.response_header(response, "Cache-Control") == "public, max-age=60, no-cache"
response = Tep::Response.new
response.append_header("Cache-Control", "public, max-age=60")
response.header_fields.push(["vArY", "Accept-Language"])
response.body = "english"
cache = Tep::ThrusterCache.new(100000)
cache.enabled = true
cache.store(cache_request("en"), response)
raise "default Vary ignored repeated collection" if response.headers.key?("Vary")
raise "field-only variant reused" if cache.answer(cache_request("fr"), Tep::Response.new)
raise "field-only variant missed" unless cache.answer(cache_request("en"), Tep::Response.new)
"#)).assert_passes();
}
