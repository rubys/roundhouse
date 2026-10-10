//! Rails' HTTP Token and Basic auth helpers (#241, #274):
//! `authenticate_with_http_token`, `authenticate_or_request_with_http_token`,
//! `authenticate_with_http_basic` and `authenticate_or_request_with_http_basic`.
//!
//! The calls were emitted as they are written, but no runtime file defined
//! them, so spinel refused the tree and the Ruby target raised NoMethodError
//! on the first request. A filter calling the `or_request` form also got no
//! halting check, so the action ran after the 401 challenge.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const WIDGET: &str = "class Widget < ApplicationRecord\nend\n";
const SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";
const ROUTES: &str = r#"Rails.application.routes.draw do
  resources :widgets, only: :index
  get "peek", to: "widgets#peek"
  get "basic", to: "widgets#basic"
end
"#;

/// An API controller guarded by a token filter, an action that only peeks
/// at the token, and one guarded by Basic auth inline.
const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  before_action :require_token, only: :index

  def index
    render plain: "widgets=#{Widget.count} bearer=#{@bearer}"
  end

  def peek
    found = authenticate_with_http_token { |token, options| token == "secret" ? "ok #{options["nonce"]}" : nil }
    render plain: "found=#{found}"
  end

  def basic
    authenticate_or_request_with_http_basic("Widgets") do |user, password|
      user == "admin" && password == "pw"
    end
    render plain: "basic ok" unless performed?
  end

  private

  def require_token
    @bearer = authenticate_or_request_with_http_token do |token, _options|
      token == "secret" ? "ok" : nil
    end
  end
end
"#;

const APP: &[(&str, &str)] = &[
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
    ("app/models/widget.rb", WIDGET),
    ("app/controllers/widgets_controller.rb", CONTROLLER),
    ("config/routes.rb", ROUTES),
    ("db/schema.rb", SCHEMA),
];

fn spinel_tree() -> Vec<(String, String)> {
    let tree: HashMap<PathBuf, Vec<u8>> = APP
        .iter()
        .map(|(path, content)| (PathBuf::from(path), content.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), BuildTarget::Spinel).expect("spinel files")
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not emitted"))
        .1
}

fn assert_parses(files: &[(String, String)], path: &str) {
    let source = file(files, path);
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

#[test]
fn the_spinel_tree_defines_the_http_auth_helpers_and_reads_the_header() {
    let files = spinel_tree();
    for path in ["app/controllers/widgets_controller.rb", "runtime/http_authentication.rb", "boot.rb", "main.rb"] {
        assert_parses(&files, path);
    }
    let runtime = file(&files, "runtime/http_authentication.rb");
    for helper in [
        "def authenticate_with_http_token",
        "def authenticate_or_request_with_http_token",
        "def request_http_token_authentication",
        "def authenticate_with_http_basic",
        "def authenticate_or_request_with_http_basic",
        "def request_http_basic_authentication",
    ] {
        assert!(runtime.contains(helper), "runtime/http_authentication.rb lacks `{helper}`");
    }
    assert!(file(&files, "runtime/http_authentication.rbs").contains("def authenticate_with_http_token:"));
    assert!(file(&files, "boot.rb").contains("require_relative \"runtime/http_authentication\""));
    // The dispatcher maps every parsed wire header into Rails' request env;
    // this must include Authorization for the auth helpers to see credentials.
    assert!(
        file(&files, "main.rb").contains(
            "request_obj.env[ActionDispatch::Http::Headers.env_name(name)] = value"
        ),
        "main.rb does not map parsed headers into the request env"
    );
}

/// The filter can answer the 401, so the inlined filter is followed by the
/// halting check, as for a filter that calls `head` or `render` itself.
#[test]
fn a_token_filter_halts_the_action_when_it_answers_401() {
    let files = spinel_tree();
    let controller = file(&files, "app/controllers/widgets_controller.rb");
    let call = controller
        .find("authenticate_or_request_with_http_token")
        .unwrap_or_else(|| panic!("the filter's call is not emitted:\n{controller}"));
    let rest = &controller[call..];
    let halt = rest.find("return if self.performed?");
    let count = rest.find("SELECT COUNT(*)");
    assert!(
        matches!((halt, count), (Some(h), Some(c)) if h < c),
        "no halting check between the token filter and the action:\n{controller}"
    );
}

#[test]
fn token_and_basic_auth_answer_401_without_credentials_and_200_with_them() {
    let mut overlay = emit_and_run::empty_app();
    for (path, content) in APP {
        overlay = overlay.write(path, content);
    }
    overlay
        .run_ruby(
            r#"def get(path, authorization = nil)
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new("") }
  env["HTTP_AUTHORIZATION"] = authorization unless authorization.nil?
  status, headers, body = Main.run_rack(env)
  [status, headers["www-authenticate"], body.join]
end

def expect(path, authorization, want)
  got = get(path, authorization)
  raise "GET #{path} with #{authorization.inspect} answered #{got.inspect}, want #{want.inspect}" unless got == want
end

token_denied = [401, 'Token realm="Application"', "HTTP Token: Access denied.\n"]
expect("/widgets", nil, token_denied)
expect("/widgets", "Bearer wrong", token_denied)
expect("/widgets", "Bearer secret", [200, nil, "widgets=0 bearer=ok"])
expect("/widgets", 'Token token="secret", nonce="n1"', [200, nil, "widgets=0 bearer=ok"])
expect("/peek", nil, [200, nil, "found="])
expect("/peek", 'Token token="secret"; nonce="n1"', [200, nil, "found=ok n1"])

basic_denied = [401, 'Basic realm="Widgets"', "HTTP Basic: Access denied.\n"]
expect("/basic", nil, basic_denied)
expect("/basic", "Basic #{["admin:wrong"].pack("m0")}", basic_denied)
expect("/basic", "Basic #{["admin:pw"].pack("m0")}", [200, nil, "basic ok"])
"#,
        )
        .assert_passes();
}
