//! rack-mini-profiler's package on the Spinel tree
//! (`runtime/gems/rack_mini_profiler/`, registered by
//! `src/gems/rack_mini_profiler.rs`). The emit half runs everywhere; the
//! native half needs a Spinel compiler and is `#[ignore]`d like its
//! sibling suites (CI's `spinel-framework` job runs it with `--ignored`,
//! `scripts/ci-plan.py`'s `SPINEL_TESTS`). The wire format was compared
//! against the real gem 4.0.0 behind the CRuby tree; these pin it.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    rack-mini-profiler (4.0.0)\n      rack (>= 1.2.0)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  rack-mini-profiler\n\nBUNDLED WITH\n   2.5.0\n";

fn app(with_gem: bool) -> emit_and_run::Overlay {
    let overlay = emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  before_action :mini_profiler\n\n  def mini_profiler\n    Rack::MiniProfiler.authorize_request\n  end\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "config/initializers/mini_profiler.rb",
            "Rack::MiniProfiler.config.disable_caching = false\nRack::MiniProfiler.config.position = \"bottom-left\"\n",
        );
    if with_gem { overlay.write("Gemfile.lock", LOCK) } else { overlay }
}

#[test]
fn the_package_ships_and_replays_the_initializer_when_the_lock_pins_the_gem() {
    let (emitted, errors) = app(true).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let boot = std::fs::read_to_string(emitted.join("boot.rb")).unwrap();
    let hooks = boot.find("require_relative \"runtime/request_hooks\"\n").expect("the hooks load");
    let package = boot
        .find("require_relative \"runtime/gems/rack_mini_profiler/rack_mini_profiler\"\n")
        .expect("the package loads");
    let setting = boot.find("Rack::MiniProfiler.config.position = \"bottom-left\"\n").expect("the setting is replayed");
    assert!(hooks < package && package < setting, "{boot}");
    for file in ["rack_mini_profiler.rb", "LICENSE", "html/includes.js", "html/includes.css", "html/vendor.js"] {
        assert!(emitted.join("runtime/gems/rack_mini_profiler").join(file).is_file(), "missing {file}");
    }
}

#[test]
fn without_the_gem_nothing_of_it_ships() {
    let (emitted, _) = app(false).emit(BuildTarget::Spinel);
    let boot = std::fs::read_to_string(emitted.join("boot.rb")).unwrap();
    assert!(!boot.contains("require_relative \"runtime/gems/"), "{boot}");
    assert!(!emitted.join("runtime/gems").exists());
}

#[test]
fn a_gemfile_require_false_keeps_the_package_out() {
    let (emitted, _) = app(true).write("Gemfile", "gem \"rack-mini-profiler\", require: false\n").emit(BuildTarget::Spinel);
    let boot = std::fs::read_to_string(emitted.join("boot.rb")).unwrap();
    assert!(!boot.contains("require_relative \"runtime/gems/"), "{boot}");
}

#[test]
fn a_setting_the_package_does_not_carry_is_reported_and_not_replayed() {
    let (emitted, errors) = app(true)
        .write(
            "config/initializers/mini_profiler.rb",
            "Rack::MiniProfiler.config.position = \"bottom-left\"\nRack::MiniProfiler.config.skip_schema_queries = true\n",
        )
        .emit(BuildTarget::Spinel);
    assert!(
        errors.iter().any(|e| e.contains("gem package setting") && e.contains("skip_schema_queries")),
        "{}",
        errors.join("\n")
    );
    if let Ok(boot) = std::fs::read_to_string(emitted.join("boot.rb")) {
        assert!(!boot.contains("skip_schema_queries"), "{boot}");
    }
}

/// Drives the hook as `Main.dispatch` does: begin, the app's response, end.
const DRIVE: &str = r##"
def mp_request(verb, path, query, cookie)
  req = Tep::Request.new
  req.verb = verb
  req.path = path
  req.raw_query = query
  req.raw_path = query.empty? ? path : path + "?" + query
  req.req_headers["host"] = "example.com:3000"
  req.cookies["__profilin"] = cookie unless cookie.empty?
  req
end

def mp_page(req, authorize)
  res = Tep::Response.new
  answered = RequestHooks.request_begin(req, res)
  unless answered
    Rack::MiniProfiler.authorize_request if authorize
    res.body = "<html><body><p>hi</p></body></html>"
  end
  RequestHooks.request_end(req, res)
  res
end

# development: every request is profiled
res = mp_page(mp_request("GET", "/", "", ""), false)
id = res.headers["X-MiniProfiler-Ids"]
raise "no ids header" if id.empty? || id.length > 20
raise "no script: #{res.body}" unless res.body.include?("<script async nonce=\"\" type=\"text/javascript\" id=\"mini-profiler\" src=\"/mini-profiler-resources/includes.js?v=e0bcc9ce0ae3bb5d6b736b6f282f601f\"")
raise "initializer not applied" unless res.body.include?("data-horizontal-position=\"left\" data-vertical-position=\"bottom\"")
raise "script not before </body>" unless res.body.include?("></script>\n</body>")
raise "cache-control" unless res.headers["Cache-Control"] == "must-revalidate, private, max-age=0"

req = mp_request("POST", "/mini-profiler-resources/results", "", "")
req.raw_body = "id=" + id + "&popup=1"
req.req_headers["x-requested-with"] = "XMLHttpRequest"
res = Tep::Response.new
raise "results not answered" unless RequestHooks.request_begin(req, res)
RequestHooks.request_end(req, res)
raise "results: #{res.status} #{res.body}" unless res.status == 200 && res.body.start_with?("{\"id\":\"" + id + "\",\"name\":\"/\"")
raise "not viewed: #{res.body}" unless res.body.include?("\"has_user_viewed\":true,\"client_timings\":null")
raise "root name: #{res.body}" unless res.body.include?("\"name\":\"GET http://example.com:3000/\"")

req = mp_request("GET", "/mini-profiler-resources/includes.css", "", "")
res = Tep::Response.new
raise "asset not answered" unless RequestHooks.request_begin(req, res)
RequestHooks.request_end(req, res)
raise "asset: #{res.file_path}" unless res.file_path == "runtime/gems/rack_mini_profiler/html/includes.css" && res.headers["Content-Type"] == "text/css"

req = mp_request("GET", "/x", "q=</script><!--", "")
res = mp_page(req, false)
sid = res.headers["X-MiniProfiler-Ids"].split(",")[0]
res = Tep::Response.new
req = mp_request("GET", "/mini-profiler-resources/results", "id=" + sid, "")
RequestHooks.request_begin(req, res)
RequestHooks.request_end(req, res)
raise "share page: #{res.body}" unless res.body.include?("var profiler = {") && !res.body.include?("q=</script>") && !res.body.include?("<!--")
raise "share page escaping: #{res.body}" unless res.body.include?("q=\\u003c/script\\u003e\\u003c!--")

# the Railtie's subscribers: the action, its template, a partial inside it, and the SQL between them
req = mp_request("GET", "/stories", "", "")
res = Tep::Response.new
RequestHooks.request_begin(req, res)
processing = ActiveSupport::Notifications.start_processing("StoriesController", "index")
sleep 0.003
template = ActiveSupport::Notifications.render_start
sleep 0.003
ActiveSupport::Notifications.sql_open("SELECT 1 FROM x WHERE a < 'b'")
ActiveSupport::Notifications.sql_close
partial = ActiveSupport::Notifications.render_start
sleep 0.003
ActiveSupport::Notifications.render_finish("render_partial.action_view", "app/views/stories/_row.html.erb", partial)
sleep 0.003
ActiveSupport::Notifications.render_finish("render_template.action_view", "app/views/stories/index.html.erb", template)
ActiveSupport::Notifications.sql_cached("SELECT 2")
ActiveSupport::Notifications.process_action("StoriesController", "index", processing)
res.body = "<html><body></body></html>"
RequestHooks.request_end(req, res)
tid = res.headers["X-MiniProfiler-Ids"].split(",")[0]
req = mp_request("POST", "/mini-profiler-resources/results", "", "")
req.raw_body = "id=" + tid
req.req_headers["x-requested-with"] = "XMLHttpRequest"
res = Tep::Response.new
RequestHooks.request_begin(req, res)
RequestHooks.request_end(req, res)
tree = res.body
raise "counts: #{tree}" unless tree.include?("\"sql_count\":2,\"cached_sql_count\":1,")
raise "executing: #{tree}" unless tree.include?("\"name\":\"Executing: stories#index\"")
row = tree.index("\"name\":\"Rendering: stories/_row.html.erb\"")
raise "no partial: #{tree}" if row.nil?
raise "partial not inside the template: #{tree}" unless tree[tree.index("\"depth\":", row), 9] == "\"depth\":3"
raise "sql text: #{tree}" unless tree.include?("\"formatted_command_string\":\"SELECT 1 FROM x WHERE a &lt; &#39;b&#39;\"")
raise "cached: #{tree}" unless tree.include?("\"cached\":true,") && tree.include?("\"cached\":null,")

# production: the cookie gates profiling, and the controller authorizes
Rack::MiniProfiler.config.apply_railtie(Rails::Env.new("production"))
res = mp_page(mp_request("GET", "/", "", ""), true)
raise "profiled without a cookie" if res.headers.key?("X-MiniProfiler-Ids")
cookie = res.set_cookies.find { |c| c.start_with?("__profilin=") }
raise "no cookie" if cookie.nil?
value = Tep::Url.unescape(cookie.split(";")[0].sub("__profilin=", ""))
raise "cookie: #{value}" unless value.start_with?("p=t,a=")
res = mp_page(mp_request("GET", "/", "", value), true)
raise "not profiled with the cookie" unless res.headers.key?("X-MiniProfiler-Ids")
res = mp_page(mp_request("GET", "/", "", value), false)
raise "profiled an unauthorized request" if res.headers.key?("X-MiniProfiler-Ids")
puts "ok"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_authorized_page_gets_the_panel_and_its_results_on_spinel() {
    let run = app(true).run_spinel(DRIVE);
    run.assert_passes();
    assert!(run.stdout.contains("ok"), "{}", run.stdout);
}
