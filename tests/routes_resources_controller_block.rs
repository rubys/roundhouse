//! #1239: `resources …, controller:` names the controller for every route
//! the resource declares, the verbs in its block included (`member { get
//! :download }`, `collection`, `on: :member`), as Rails routes them. Before,
//! only the standard actions took it, and each block verb dispatched to a
//! controller named after the resource. A nested `resources` keeps its
//! own controller, and path and helper names are unchanged.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use std::process::Command;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::routes::flatten_routes;

const ROUTES: &str = r#"  resources :downloads, only: [:show], controller: :exports do
    member { get :download }
    collection { get :recent }
    get :inline, on: :member
    resources :parts, only: :index
  end
  resources :messages, controller: "messages/by_bots", only: [] do
    member { get :ping }
  end
"#;

/// `VERB path controller#action helper`, as actionpack 8.1.4 lists them
/// (`RouteSet#routes`), with the controller as Rails names it.
const RAILS: &[&str] = &[
    "GET /downloads/:id/download exports#download download_download",
    "GET /downloads/recent exports#recent recent_downloads",
    "GET /downloads/:id/inline exports#inline inline_download",
    "GET /downloads/:download_id/parts parts#index download_parts",
    "GET /downloads/:id exports#show download",
    "GET /messages/:id/ping messages/by_bots#ping ping_message",
];

/// The lowered routes in the same shape. A controller class reads back
/// as Rails' path (`Messages::ByBotsController` is `messages/by_bots`).
fn lowered() -> Vec<String> {
    let tree = vec![(
        std::path::PathBuf::from("config/routes.rb"),
        format!("Rails.application.routes.draw do\n{ROUTES}end\n").into_bytes(),
    )]
    .into_iter()
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let mut rows: Vec<String> = flatten_routes(&app)
        .into_iter()
        .map(|r| {
            let class = r.controller.0.as_str().trim_end_matches("Controller");
            let controller = class.split("::").map(roundhouse::naming::snake_case).collect::<Vec<_>>().join("/");
            format!(
                "{} {} {}#{} {}",
                format!("{:?}", r.method).to_uppercase(),
                r.path,
                controller,
                r.action.as_str(),
                r.as_name
            )
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn block_verbs_dispatch_to_the_resources_controller() {
    let mut want: Vec<String> = RAILS.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(lowered(), want);
}

/// The table itself, against actionpack 8.1.4's router.
#[test]
fn rails_routes_the_table() {
    if !Command::new("ruby").args(["-raction_controller", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the actionpack gem not available");
        return;
    }
    let script = format!(
        r##"gem "actionpack", "8.1.4"
require "action_controller"
rs = ActionDispatch::Routing::RouteSet.new
rs.draw do
{ROUTES}end
rows = rs.routes.map {{ |r| [r.verb, r.path.spec.to_s.sub("(.:format)", ""), "#{{r.defaults[:controller]}}##{{r.defaults[:action]}}", r.name].join(" ") }}
want = {want:?}
raise "Rails routes #{{rows.sort.inspect}}" unless rows.sort == want.sort
puts "Rails agrees"
"##,
        want = RAILS.to_vec(),
    );
    let output = Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Rails agrees"),
        "actionpack 8.1.4 disagrees with the table\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}

/// A request to a block verb reaches `controller:`'s action on the Ruby
/// target (it was routed to a `DownloadsController` the app does not have).
#[test]
fn the_ruby_target_dispatches_a_block_verb_to_the_named_controller() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write(
            "app/controllers/exports_controller.rb",
            "class ExportsController < ApplicationController\n  def show\n    render plain: \"show:#{params[:id]}\"\n  end\n\n  def download\n    render plain: \"download:#{params[:id]}\"\n  end\n\n  def recent\n    render plain: \"recent\"\n  end\n\n  def inline\n    render plain: \"inline:#{params[:id]}\"\n  end\nend\n",
        )
        .write("app/controllers/parts_controller.rb", "class PartsController < ApplicationController\n  def index\n    render plain: \"parts:#{params[:download_id]}\"\n  end\nend\n")
        .write("app/controllers/messages/by_bots_controller.rb", "class Messages::ByBotsController < ApplicationController\n  def ping\n    render plain: \"ping:#{params[:id]}\"\n  end\nend\n")
        .write("config/routes.rb", &format!("Rails.application.routes.draw do\n{ROUTES}end\n"))
        .run_ruby(
            r##"require "stringio"
def req(verb, path)
  result = Main.run_rack("REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  result[0].to_s + "|" + result[2].join
end
{ "/downloads/1/download" => "200|download:1", "/downloads/recent" => "200|recent", "/downloads/1/inline" => "200|inline:1",
  "/downloads/1/parts" => "200|parts:1", "/downloads/1" => "200|show:1" }.each do |path, want|
  got = req("GET", path)
  raise "#{path}: #{got}" unless got == want
end
got = req("GET", "/messages/1/ping")
raise "ping: #{got}" unless got == "200|ping:1"
puts "PASS"
"##,
        )
        .assert_passes();
}

