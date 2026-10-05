//! `fixtures/tiny-api`: the API-only app class (#321).
//!
//! No other fixture is this kind of app. real-blog is HTML on
//! `ActionController::Base` with views and a `root`; tiny-blog-uuid is
//! the same shape with uuid keys. A Rails API app differs in ancestry
//! (`ApplicationController < ActionController::API`), in topology (no
//! `app/views`, no `root`, `resources … only:`) and in how it answers
//! (inline `render json:` of a Hash or an array of summary Hashes). The
//! fixture also keeps uuid keys, an enum, a concern method with
//! optional, rest and keyword parameters, a nested class and a keyword
//! helper on the controller.
//!
//! Three layers:
//!
//! - Always on, no toolchain: ingest, `analyze_and_lower` with zero
//!   error diagnostics, then the Spinel and Ruby trees emit with no
//!   emission error, contain the app's files and parse with prism.
//!   That says the app is emitted, not that it runs.
//! - CRuby request lane, `#[ignore]`d for the toolchain only: the Ruby
//!   tree booted on CRuby and driven through `Main.run_rack`. Tests that
//!   pass are named `cruby_gate_…`, and CI's compare-ruby job selects
//!   them by that prefix:
//!
//!       cargo test --test tiny_api cruby_gate_ -- --ignored --nocapture
//!
//!   A test that states the intended behaviour but fails on main is
//!   named after its issue and stays out of that selection. When the
//!   issue is fixed, rename it to `cruby_gate_…` and keep the ignore.
//! - Spinel lane, `#[ignore]`d for the toolchain only and split the
//!   same way: the Spinel tree's app entry built with `spin build` and
//!   sent HTTP requests, and the model script compiled natively.
//!   CI's toolchain-spinel job selects `spinel_gate_…`:
//!
//!       cargo test --test tiny_api spinel_gate_ -- --ignored --nocapture

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

use roundhouse::App;
use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app;
use roundhouse::project::{BuildTarget, target_files};

/// Files every emitted tree must contain. Parsing what was emitted
/// cannot notice a controller or model that was never written.
const APP_FILES: &[&str] = &[
    "main.rb",
    "app/controllers/application_controller.rb",
    "app/controllers/widgets_controller.rb",
    "app/controllers/formats_controller.rb",
    "app/models/application_record.rb",
    "app/models/widget.rb",
    "app/models/widget/invalid.rb",
    "app/models/part.rb",
    "app/models/labelled.rb",
    "config/routes.rb",
];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-api")
}

/// The ingested, analyzed and lowered fixture, with the error
/// diagnostics of both passes formatted for an assertion message.
fn lowered() -> (App, Vec<String>) {
    let mut app = ingest_app(&fixture()).expect("ingest fixtures/tiny-api");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    let mut errors = Vec::new();
    let mut warnings = 0usize;
    for d in diagnose(&app).iter().chain(&lower_diags) {
        if d.severity == Severity::Error {
            errors.push(format!("{:?}: {}", d.span, d.message));
        } else {
            warnings += 1;
        }
    }
    // Warnings are modeling debt, not a failure; print the count so a
    // run with --nocapture records it instead of hiding it.
    eprintln!("tiny-api: {warnings} analysis/lowering warning(s)");
    (app, errors)
}

/// `target`'s emitted tree and its emission error diagnostics.
fn emit(app: &App, target: BuildTarget) -> (Vec<(String, String)>, Vec<String>) {
    let (files, diags) =
        roundhouse::emit::diagnostics::scope(|| target_files(app, &fixture(), target));
    let files = files.unwrap_or_else(|e| panic!("{target:?} target files: {e}"));
    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{:?}: {}", d.span, d.message))
        .collect();
    (files, errors)
}

fn assert_no_errors(stage: &str, errors: &[String]) {
    assert!(
        errors.is_empty(),
        "{stage} reports errors:\n{}",
        errors.join("\n")
    );
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("{path} not emitted"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(fixture().join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The dimensions that make this fixture an API app. A drive-by edit
/// that makes requests green by flipping the parent to `Base`, adding a
/// view or a `root`, or dropping the uuid keys fails here first.
#[test]
fn the_fixture_keeps_its_api_only_shape() {
    let controller = read("app/controllers/application_controller.rb");
    assert!(
        controller.contains("class ApplicationController < ActionController::API\n"),
        "{controller}"
    );
    assert!(
        !fixture().join("app/views").exists(),
        "an API app has no app/views"
    );
    let routes = read("config/routes.rb");
    assert!(
        !routes.lines().any(|l| l.trim_start().starts_with("root")),
        "an API app declares no root:\n{routes}"
    );
    let schema = read("db/schema.rb");
    for table in ["widgets", "parts"] {
        assert!(
            schema.contains(&format!("create_table \"{table}\", id: :uuid")),
            "{table} is not uuid-keyed:\n{schema}"
        );
    }
}

#[test]
fn analysis_and_lowering_report_no_errors() {
    let (_, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
}

#[test]
fn spinel_and_ruby_emit_the_app_and_every_file_parses() {
    let (app, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let (files, errors) = emit(&app, target);
        assert_no_errors(&format!("{target:?} emission"), &errors);
        for path in APP_FILES {
            file(&files, path);
        }
        assert!(
            !files.iter().any(|(p, _)| p.starts_with("app/views/")),
            "{target:?} emitted a view for an app that has none"
        );
        // The parent is the point of the fixture; the emit must not
        // quietly substitute `Base` for it.
        let controller = file(&files, "app/controllers/application_controller.rb");
        assert!(
            controller.contains("class ApplicationController < ActionController::API\n"),
            "{target:?}:\n{controller}"
        );
        for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
            let result = ruby_prism::parse(source.as_bytes());
            let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
            assert!(
                errors.is_empty(),
                "{target:?} {path} does not parse: {errors:?}\n{source}"
            );
        }
    }
}

/// `Widget::Invalid` gets its own file and `.rbs` sidecar in the
/// Spinel tree, and the sidecar declares it with its superclass.
#[test]
fn the_nested_class_is_declared_in_its_spinel_sidecar() {
    let (app, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
    let (files, _) = emit(&app, BuildTarget::Spinel);
    let rbs = file(&files, "app/models/widget/invalid.rbs");
    assert!(rbs.contains("class Invalid < StandardError\n"), "{rbs}");
    if let Err(e) = ruby_rbs::node::parse(rbs) {
        panic!("app/models/widget/invalid.rbs is not valid RBS: {e}\n{rbs}");
    }
}

/// Emit the Ruby tree into a fresh directory, then run `script` there on
/// CRuby with the scaffold's bundle, after `main.rb` is required and the
/// default adapter is configured on an in-memory database. The directory
/// is removed when the script succeeds and kept, for reading, when it
/// fails.
fn run_on_cruby(name: &str, script: &str) -> (Output, PathBuf) {
    let (app, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
    let (files, errors) = emit(&app, BuildTarget::Ruby);
    assert_no_errors("Ruby emission", &errors);

    let scratch =
        std::env::temp_dir().join(format!("roundhouse-tiny-api-{name}-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    roundhouse::project::write_to_dir(&files, &scratch).expect("write the Ruby tree");
    let script = format!(
        "require File.expand_path(\"main\", Dir.pwd)\nMain.configure_default_adapter!\n{REQUEST}{script}"
    );
    std::fs::write(scratch.join("tiny_api_probe.rb"), script).expect("write the probe");

    let gemfile = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel/scaffold/Gemfile");
    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", gemfile)
        .env("BLOG_DB", ":memory:")
        .args(["exec", "ruby", "-I.", "tiny_api_probe.rb"])
        .current_dir(&scratch)
        .output()
        .expect("spawn bundle exec ruby");
    (output, scratch)
}

fn assert_ran(output: &Output, scratch: &Path, marker: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(marker),
        "the probe failed in {}\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    // Only a passing run is cleaned up; a failing one is the evidence.
    let _ = std::fs::remove_dir_all(scratch);
}

/// A JSON request through the Rack entry point, as Puma would send it.
/// Returns the status, the content type and the whole body.
const REQUEST: &str = r#"require "json"
def request(method, path, body = "", query = "")
  status, headers, chunks = Main.run_rack(
    "REQUEST_METHOD" => method, "PATH_INFO" => path, "QUERY_STRING" => query,
    "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s,
    "rack.input" => StringIO.new(body))
  text = +""
  chunks.each { |chunk| text << chunk }
  [status, headers["content-type"], text]
end
"#;

/// The app boots, and a path no route matches answers 404, `/`
/// included because the app has no `root`. No controller is loaded on
/// this path, which is why it already holds while #163 is open.
#[test]
#[ignore = "requires CRuby + scaffold bundle"]
fn cruby_gate_unrouted_paths_answer_404() {
    let (output, scratch) = run_on_cruby(
        "unrouted",
        r#"["/", "/parts", "/widgets/1/parts"].each do |path|
  status, = request("GET", path)
  raise "GET #{path} answered #{status}, want 404" unless status == 404
end
puts "TINY API UNROUTED OK"
"#,
    );
    assert_ran(&output, &scratch, "TINY API UNROUTED OK");
}

/// Literal JSON paths, formatted resources, and unknown routes use the shared router.
#[test]
#[ignore = "requires CRuby + scaffold bundle"]
fn cruby_gate_literal_json_routes_match() {
    let (output, scratch) = run_on_cruby(
        "literal-json-routes",
        r#"{
  "/feed.json" => "literal",
  "/formats" => "collection",
  "/formats.json" => "collection",
  "/formats/42.json" => "42"
}.each do |path, expected|
  status, _, body = request("GET", path)
  raise "GET #{path}: #{status} #{body}" unless status == 200 && body == expected
end
status, = request("GET", "/missing.json")
raise "unknown JSON route matched" unless status == 404
puts "LITERAL JSON ROUTES OK"
"#,
    );
    assert_ran(&output, &scratch, "LITERAL JSON ROUTES OK");
}

/// The models the controller serves, driven directly: a minted uuid
/// key, the enum, `has_many` across a uuid foreign key, the concern
/// method's optional, rest and keyword parameters, a rest-and-block
/// method, the nested error class, and the summary Hash the controller
/// renders. The same script runs on CRuby and, compiled, on Spinel.
const MODELS: &str = r#"widget = Widget.new(name: "gear")
raise "save failed" unless widget.save
raise "no uuid minted: #{widget.id.inspect}" unless widget.id =~ /\A[0-9a-f-]{36}\z/
raise "enum default: #{widget.status.inspect}" unless widget.status == "draft"
widget.status = :live
widget.save
raise "enum write did not persist" unless Widget.find(widget.id).status == "live"
raise "blank name must not save" if Widget.new(name: "").save

%w[axle hub].each do |name|
  part = Part.new(name: name)
  part.widget = widget
  raise "part #{name} did not save" unless part.save
end
raise "has_many count across a uuid fk" unless widget.parts.count == 2

seen = []
widget.each_part("hub") { |part| seen << part.name }
raise "each_part yielded #{seen.inspect}" unless seen == ["hub"]

raise "label()" unless widget.label == "gear"
raise "label(prefix)" unless widget.label("big") == "big gear"
raise "label(prefix, *parts, separator:)" unless widget.label("big", "red", separator: "-") == "big-gear-red"

begin
  raise Widget::Invalid, "bad widget"
rescue Widget::Invalid => e
  raise "nested class lost its parent" unless e.is_a?(StandardError) && e.message == "bad widget"
end

summary = widget.summary
want = { id: widget.id, name: "gear", status: "live", parts: 2 }
raise "summary #{summary.inspect}" unless summary == want
puts "TINY API MODELS OK"
"#;

#[test]
#[ignore = "requires CRuby + scaffold bundle"]
fn cruby_gate_models_run() {
    let (output, scratch) = run_on_cruby("models", MODELS);
    assert_ran(&output, &scratch, "TINY API MODELS OK");
}

/// The routed JSON actions on the `ActionController::API` parent:
/// create, the validation error, index, show, the not-found branch and
/// update and its validation error, each an inline `render json:` of a Hash or an array of
/// summary Hashes. On main the first routed request raises
/// `uninitialized constant ActionController::API` (NameError) when the
/// controller loads, because the Ruby runtime defines only `Base`
/// (#163). When that is fixed, rename this `cruby_gate_…` so CI runs it.
#[test]
#[ignore = "#163: ActionController::API is undefined in the Ruby runtime; also requires CRuby + scaffold bundle"]
fn issue_163_api_controllers_answer_json_requests() {
    let (output, scratch) = run_on_cruby(
        "issue-163",
        r##"def json(method, path, want_status, body = "", query = "")
  status, type, text = request(method, path, body, query)
  raise "#{method} #{path} answered #{status}, want #{want_status}: #{text}" unless status == want_status
  raise "#{method} #{path} content type #{type.inspect}" unless type.to_s.start_with?("application/json")
  JSON.parse(text)
end

created = json("POST", "/widgets", 201, { name: "gear" }.to_json)
id = created["id"]
raise "no uuid in #{created.inspect}" unless id =~ /\A[0-9a-f-]{36}\z/
raise "create body #{created.inspect}" unless created == { "id" => id, "name" => "gear", "status" => "draft", "parts" => 0 }

problem = json("POST", "/widgets", 422, { name: "" }.to_json)
raise "422 body #{problem.inspect}" unless problem == { "error" => "Name can't be blank" }

json("POST", "/widgets", 201, { name: "axle" }.to_json)
listed = json("GET", "/widgets", 200)
raise "index #{listed.inspect}" unless listed.map { |w| w["name"] } == %w[axle gear]
paged = json("GET", "/widgets", 200, "", "per=1")
raise "per=1 #{paged.inspect}" unless paged.map { |w| w["name"] } == %w[axle]

shown = json("GET", "/widgets/#{id}", 200)
raise "show #{shown.inspect}" unless shown == created
missing = json("GET", "/widgets/00000000-0000-4000-8000-000000000000", 404)
raise "404 body #{missing.inspect}" unless missing == { "error" => "not found" }

updated = json("PATCH", "/widgets/#{id}", 200, { name: "cog" }.to_json)
raise "update #{updated.inspect}" unless updated["name"] == "cog"
raise "update did not persist" unless json("GET", "/widgets/#{id}", 200)["name"] == "cog"
rejected = json("PATCH", "/widgets/#{id}", 422, { name: "" }.to_json)
raise "update 422 body #{rejected.inspect}" unless rejected == { "error" => "Name can't be blank" }
raise "a rejected update persisted" unless json("GET", "/widgets/#{id}", 200)["name"] == "cog"
puts "TINY API REQUESTS OK"
"##,
    );
    assert_ran(&output, &scratch, "TINY API REQUESTS OK");
}

// The Spinel lane: the Spinel tree emitted, compiled ahead of time and
// run. `spinel_gate_…` tests pass on main and CI's toolchain-spinel job
// selects them by that prefix:
//
//     cargo test --test tiny_api spinel_gate_ -- --ignored --nocapture
//
// As on CRuby, a test that states the intended behaviour but fails on
// main is named after its issue and stays out of that selection.

/// Emit the Spinel tree into a fresh directory. Like the CRuby lane's,
/// it is removed by a passing test and kept by a failing one.
fn emit_spinel(name: &str) -> PathBuf {
    let (app, errors) = lowered();
    assert_no_errors("analysis and lowering", &errors);
    let (files, errors) = emit(&app, BuildTarget::Spinel);
    assert_no_errors("Spinel emission", &errors);

    let scratch = std::env::temp_dir().join(format!(
        "roundhouse-tiny-api-spinel-{name}-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    roundhouse::project::write_to_dir(&files, &scratch).expect("write the Spinel tree");
    scratch
}

/// Run a compile step in `dir`, panic with its output when it fails,
/// and print how long it took.
fn compile(dir: &Path, program: &str, args: &[&str]) {
    let started = Instant::now();
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("spawn {program}: {e}"));
    assert!(
        output.status.success(),
        "`{program} {}` failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!(
        "tiny-api: `{program} {}` took {:.1}s",
        args.join(" "),
        started.elapsed().as_secs_f64()
    );
}

/// The app entry, `bin/blog.rb`, built by `spin build` into
/// `build/bin/blog`: the binary a user of the Spinel tree runs.
fn build_app(name: &str) -> PathBuf {
    let scratch = emit_spinel(name);
    compile(&scratch, "spin", &["build", "blog"]);
    assert!(
        scratch.join("build/bin/blog").is_file(),
        "spin build left no build/bin/blog in {}",
        scratch.display()
    );
    scratch
}

/// The built app serving HTTP on a free local port, with one OS worker
/// and a fresh SQLite file in the tree (not `:memory:`: the boot creates
/// the schema on one connection and requests are served on others).
/// Its output goes to `server.log` in the tree; the process is killed
/// when this is dropped.
struct Server {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Server {
    fn start(scratch: &Path) -> Server {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("pick a free port")
            .port();
        let log = scratch.join("server.log");
        let out = std::fs::File::create(&log).expect("create server.log");
        let err = out.try_clone().expect("clone server.log");
        let child = Command::new(scratch.join("build/bin/blog"))
            .current_dir(scratch)
            .env("PORT", port.to_string())
            .env("BLOG_DB", scratch.join("tiny_api.sqlite3"))
            .env("SPINEL_WORKERS", "1")
            .stdout(out)
            .stderr(err)
            .spawn()
            .expect("start build/bin/blog");
        let mut server = Server { child, port, log };
        let deadline = Instant::now() + Duration::from_secs(30);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            if let Some(status) = server.child.try_wait().expect("poll the server") {
                panic!(
                    "the server exited ({status}) before listening:\n{}",
                    server.log()
                );
            }
            assert!(
                Instant::now() < deadline,
                "the server did not listen on {port} within 30s:\n{}",
                server.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        server
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// One HTTP/1.1 request with a JSON body, on its own connection.
    /// Returns the status, the content type and the body.
    fn request(&self, method: &str, path: &str, body: &str) -> (u16, String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("read timeout");
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("send the request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .unwrap_or_else(|e| panic!("{method} {path}: {e}\n{}", self.log()));
        let response = String::from_utf8_lossy(&response).into_owned();
        let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("{method} {path}: no status line in {response:?}"));
        let content_type = head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.trim().to_string())
            .unwrap_or_default();
        (status, content_type, body.to_string())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Native HTTP dispatch preserves literal JSON routes and ordinary format suffixes.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn spinel_gate_literal_json_routes_match() {
    let scratch = build_app("literal-json-routes");
    let server = Server::start(&scratch);
    for (path, expected) in [
        ("/feed.json", "literal"),
        ("/formats", "collection"),
        ("/formats.json", "collection"),
        ("/formats/42.json", "42"),
    ] {
        let (status, _, body) = server.request("GET", path, "");
        assert_eq!((status, body.as_str()), (200, expected), "GET {path}: {}", server.log());
    }
    assert_eq!(server.request("GET", "/missing.json", "").0, 404);
    drop(server);
    std::fs::remove_dir_all(scratch).expect("clean passing tree");
}

/// The app entry builds with `spin build`, the binary boots and serves
/// HTTP, and a path no route matches answers 404, `/` included because
/// the app has no `root`. The CRuby gate of the same name says the
/// same; no controller is reached on this path, which is why it already
/// holds while #163 is open.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn spinel_gate_unrouted_paths_answer_404() {
    let scratch = build_app("unrouted");
    let server = Server::start(&scratch);
    for path in ["/", "/parts", "/widgets/1/parts"] {
        let (status, _, body) = server.request("GET", path, "");
        assert_eq!(
            status,
            404,
            "GET {path}: {body}\nkept in {}\n=== server.log ===\n{}",
            scratch.display(),
            server.log()
        );
    }
    drop(server);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The CRuby lane's model script, compiled by Spinel against the
/// emitted app's boot chain on an in-memory database and run as a
/// native binary. A library consumer, like `emit_and_run`'s
/// `run_spinel`, not a request.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn spinel_gate_models_run() {
    let scratch = emit_spinel("models");
    let contract = format!(
        "require_relative \"boot\"\nDb.configure(\":memory:\")\n\
         Schema.statements.each {{ |sql| Db.exec(sql) }}\n\
         ActiveRecord.adapter = SqliteAdapter\n{MODELS}"
    );
    std::fs::write(scratch.join("contract.rb"), contract).expect("write contract.rb");
    let spinel = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    compile(&scratch, &spinel, &["contract.rb", "-o", "contract"]);
    let output = Command::new(scratch.join("contract"))
        .current_dir(&scratch)
        .output()
        .expect("run contract");
    assert_ran(&output, &scratch, "TINY API MODELS OK");
}

/// The CRuby lane's routed JSON requests, sent over HTTP to the built
/// binary. On main every routed request answers 500 and the server
/// logs `NoMethodError: undefined method 'params=' for an instance of
/// WidgetsController`: the controllers inherit from
/// `ActionController::API`, which the dispatcher's seat is not defined
/// on (#163). Past that, the inline `render json:` lowers to
/// `ActionController::JsonRender`, which `spin build` warns is defined
/// nowhere in the Spinel tree: with #338's diff applied, the same
/// request answers 500 with `NameError: uninitialized constant
/// ActionController::JsonRender`. When both are fixed, rename this
/// `spinel_gate_…` so CI runs it.
#[test]
#[ignore = "#163, then the Spinel `render json:` fallback (ActionController::JsonRender undefined): routed requests answer 500; also requires the Spinel toolchain"]
fn issue_163_spinel_api_controllers_answer_json_requests() {
    let scratch = build_app("issue-163");
    let server = Server::start(&scratch);
    let json = |method: &str, path: &str, want: u16, body: &str| -> serde_json::Value {
        let (status, content_type, text) = server.request(method, path, body);
        assert!(
            status == want && content_type.starts_with("application/json"),
            "{method} {path} answered {status} ({content_type:?}), want {want} JSON: {text}\n\
             kept in {}\n=== server.log ===\n{}",
            scratch.display(),
            server.log()
        );
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} {path}: {e}: {text}"))
    };

    let created = json("POST", "/widgets", 201, r#"{"name":"gear"}"#);
    let id = created["id"].as_str().expect("an id").to_string();
    assert!(
        id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "no uuid in {created}"
    );
    assert_eq!(
        created,
        serde_json::json!({ "id": id, "name": "gear", "status": "draft", "parts": 0 })
    );

    let problem = json("POST", "/widgets", 422, r#"{"name":""}"#);
    assert_eq!(
        problem,
        serde_json::json!({ "error": "Name can't be blank" })
    );

    json("POST", "/widgets", 201, r#"{"name":"axle"}"#);
    let names = |list: serde_json::Value| -> Vec<String> {
        let list = list.as_array().cloned().unwrap_or_default();
        list.iter()
            .map(|w| w["name"].as_str().unwrap_or("").to_string())
            .collect()
    };
    assert_eq!(names(json("GET", "/widgets", 200, "")), ["axle", "gear"]);
    assert_eq!(names(json("GET", "/widgets?per=1", 200, "")), ["axle"]);

    assert_eq!(json("GET", &format!("/widgets/{id}"), 200, ""), created);
    let missing = json(
        "GET",
        "/widgets/00000000-0000-4000-8000-000000000000",
        404,
        "",
    );
    assert_eq!(missing, serde_json::json!({ "error": "not found" }));

    let path = format!("/widgets/{id}");
    assert_eq!(
        json("PATCH", &path, 200, r#"{"name":"cog"}"#)["name"],
        "cog"
    );
    assert_eq!(json("GET", &path, 200, "")["name"], "cog");

    drop(server);
    let _ = std::fs::remove_dir_all(&scratch);
}
