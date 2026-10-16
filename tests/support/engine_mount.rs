//! Generic source-available isolated engine fixture shared by the CRuby
//! emit-and-run check and the native Spinel HTTP witness.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use roundhouse::diagnostic::Diagnostic;
use roundhouse::project::BuildTarget;

use super::emit_and_run::{self, Overlay};

/// A host app and isolated Catalog engine with overlapping controller
/// names and paths both before and after the mount.
pub fn overlay() -> Overlay {
    emit_and_run::empty_app()
        .write(
            "Gemfile.lock",
            "PATH\n  remote: vendor/catalog\n  specs:\n    catalog (0.1.0)\n\nDEPENDENCIES\n  catalog!\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define do\n  create_table \"engine_mount_probes\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/host_controller.rb",
            r#"class HostController < ApplicationController
  def index
    render plain: "host root"
  end

  def root_helper
    render plain: root_path
  end

  def products_helper
    render plain: products_path
  end

  def after
    render plain: "host after"
  end
end
"#,
        )
        .write(
            "app/controllers/products_controller.rb",
            "class ProductsController < ApplicationController\n  def index\n    render plain: \"host products\"\n  end\nend\n",
        )
        .write(
            "app/controllers/items_controller.rb",
            "class ItemsController < ApplicationController\n  def index\n    render plain: \"host items\"\n  end\nend\n",
        )
        .write(
            "vendor/catalog/lib/catalog/engine.rb",
            "module Catalog\n  class Engine < ::Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n",
        )
        .write(
            "vendor/catalog/app/controllers/catalog/home_controller.rb",
            "module Catalog\n  class HomeController < ActionController::Base\n    def index\n      render plain: \"engine root\"\n    end\n  end\nend\n",
        )
        .write(
            "vendor/catalog/app/controllers/catalog/products_controller.rb",
            "module Catalog\n  class ProductsController < ActionController::Base\n    def index\n      render plain: \"engine products\"\n    end\n  end\nend\n",
        )
        .write(
            "vendor/catalog/app/controllers/catalog/items_controller.rb",
            "module Catalog\n  class ItemsController < ActionController::Base\n    def index\n      render plain: \"engine items\"\n    end\n  end\nend\n",
        )
        .write(
            "vendor/catalog/config/routes.rb",
            "Catalog::Engine.routes.draw do\n  root to: \"home#index\"\n  get \"/products\", to: \"products#index\"\n  get \"/items\", to: \"items#index\"\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"host#index\"\n  get \"/root-helper\", to: \"host#root_helper\"\n  get \"/products-helper\", to: \"host#products_helper\"\n  get \"/products\", to: \"products#index\", as: :products\n  get \"/catalog/products\", to: \"products#index\", as: :host_catalog_products\n  mount Catalog::Engine, at: \"/catalog\"\n  get \"/catalog/items\", to: \"items#index\", as: :host_catalog_items\n  get \"/catalog/unmatched\", to: \"host#after\"\n  get \"/after\", to: \"host#after\"\nend\n",
        )
}

/// Resolve provenance claims through the app's FileId catalogue instead of
/// inspecting the debug rendering of a span.
pub fn has_error_in_source(
    app: &roundhouse::App,
    errors: &[Diagnostic],
    path_suffix: &str,
    message_fragment: &str,
) -> bool {
    errors.iter().any(|diagnostic| {
        diagnostic.message.contains(message_fragment)
            && roundhouse::ide::source(app, diagnostic.span.file)
                .is_some_and(|source| Path::new(&source.path).ends_with(path_suffix))
    })
}

/// Match a diagnostic to source text resolved through the app's source map,
/// checking both its owning file and the line at its span.
pub fn has_error_on_source_line(
    app: &roundhouse::App,
    errors: &[Diagnostic],
    path_suffix: &str,
    line_fragment: &str,
) -> bool {
    errors.iter().any(|diagnostic| {
        roundhouse::ide::source(app, diagnostic.span.file).is_some_and(|source| {
            Path::new(&source.path).ends_with(path_suffix)
                && source
                    .text
                    .lines()
                    .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                    .is_some_and(|line| line.contains(line_fragment))
        })
    })
}

/// Render diagnostics with their resolved source paths for test failure output.
pub fn describe_errors(app: &roundhouse::App, errors: &[Diagnostic]) -> String {
    errors
        .iter()
        .map(|diagnostic| {
            let path = roundhouse::ide::source(app, diagnostic.span.file)
                .map(|source| source.path.as_str())
                .unwrap_or("<no source>");
            format!("{path}: {}", diagnostic.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Assert the application through its emitted CRuby Rack stack.
pub const CRUBY_ASSERTIONS: &str = r#"
require "rack"
require "rack/mock"
builder = Rack::Builder.new
builder.instance_eval(File.read("config.ru"), "config.ru")
client = Rack::MockRequest.new(builder.to_app)
{
  "/" => [200, "host root"],
  "/root-helper" => [200, "/"],
  "/products-helper" => [200, "/products"],
  "/products" => [200, "host products"],
  "/catalog" => [200, "engine root"],
  "/catalog/" => [200, "engine root"],
  "/catalog/products" => [200, "host products"],
  "/catalog/items" => [200, "engine items"],
  "/catalog/unmatched" => [200, "host after"],
  "/after" => [200, "host after"],
}.each do |path, (status, body)|
  response = client.get(path, "HTTP_HOST" => "example.test")
  raise "GET #{path}: #{response.status} #{response.body.inspect}" unless [response.status, response.body] == [status, body]
end
missing = client.get("/catalogue/items", "HTTP_HOST" => "example.test")
raise "mount prefix matched /catalogue: #{missing.status}" unless missing.status == 404
raise "engine products helper replaced host helper" unless RouteHelpers.products_path == "/products"
puts "literal engine mount CRuby HTTP contract passed"
"#;

/// Build the same generic fixture as a native Spinel app and exercise its
/// actual HTTP server on an ephemeral loopback port.
pub fn spinel_http_witness() {
    let (emitted, errors) = overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis or emission reported errors:\n{}", errors.join("\n"));
    let spin = std::env::var("SPIN").unwrap_or_else(|_| "spin".into());
    let build = Command::new(&spin)
        .args(["build", "blog"])
        .current_dir(&emitted)
        .output()
        .expect("spawn spin build");
    assert!(
        build.status.success(),
        "spin build blog failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(emitted.join("build/bin/blog").is_file(), "Spinel app binary missing");
    let server = Server::start(&emitted);
    for (path, expected) in [
        ("/", "host root"),
        ("/root-helper", "/"),
        ("/products-helper", "/products"),
        ("/products", "host products"),
        ("/catalog", "engine root"),
        ("/catalog/", "engine root"),
        ("/catalog/products", "host products"),
        ("/catalog/items", "engine items"),
        ("/catalog/unmatched", "host after"),
        ("/after", "host after"),
    ] {
        let (status, body) = server.get(path);
        assert_eq!((status, body.as_str()), (200, expected), "GET {path}: {}", server.log());
    }
    let (status, _) = server.get("/catalogue/items");
    assert_eq!(status, 404, "mount prefix boundary: {}", server.log());
    drop(server);
}

struct Server {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Server {
    /// Start the emitted binary on a free loopback port and wait for its HTTP
    /// listener, failing with the captured server log on startup errors.
    fn start(emitted: &Path) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("pick a free HTTP port")
            .port();
        let log = emitted.join("server.log");
        let stdout = std::fs::File::create(&log).expect("create server log");
        let stderr = stdout.try_clone().expect("clone server log");
        let child = Command::new(emitted.join("build/bin/blog"))
            .current_dir(emitted)
            .env("PORT", port.to_string())
            .env("BLOG_DB", emitted.join("engine-mount.sqlite3"))
            .env("SPINEL_WORKERS", "1")
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("start Spinel HTTP app");
        let mut server = Self { child, port, log };
        let deadline = Instant::now() + Duration::from_secs(30);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            if let Some(status) = server.child.try_wait().expect("poll server") {
                panic!("server exited ({status}) before listening:\n{}", server.log());
            }
            assert!(Instant::now() < deadline, "server did not listen:\n{}", server.log());
            std::thread::sleep(Duration::from_millis(100));
        }
        server
    }

    /// Send one close-after-response HTTP/1.1 GET and return its status/body.
    fn get(&self, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect HTTP");
        stream.set_read_timeout(Some(Duration::from_secs(30))).expect("set read timeout");
        write!(stream, "GET {path} HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n")
            .expect("write HTTP request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read HTTP response");
        let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = head.split_whitespace().nth(1).and_then(|text| text.parse().ok())
            .unwrap_or_else(|| panic!("invalid HTTP response: {response:?}"));
        (status, body.to_string())
    }

    /// Read captured stdout and stderr to explain native HTTP failures.
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
