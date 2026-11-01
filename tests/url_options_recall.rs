//! A `url_for` options hash resolves a segment it leaves out from the
//! current request, as Rails recalls it.
//!
//! lobsters paginates every story list with `link_to …, {controller:
//! controller_name, action: action_name, page: @page + 1}`. On
//! `get "/top(/:length(/page/:page))" => "home#top"` the only route
//! carrying `:page` also needs `:length`, which the hash never names:
//! Rails fills it from the request's path parameters, and when the
//! request has none (`/top?length=1y`, a QUERY parameter) it drops the
//! optional `:page` and answers `/top`. The resolver used to know only
//! routes whose segments are exactly the hash's keys, and raised
//! "no route matches home#top" — a 500 on /top?length=1y, the one
//! period long enough to have a second page.
//!
//! The expected paths are actionpack 8.1's answers for the same route
//! set and recall (`RouteSet#url_for(…, _recall:)`).
//!
//! Skips when `ruby` is not on the machine; the CI core job has it.

use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::BuildTarget;

fn tree() -> PathBuf {
    tree_for(BuildTarget::Ruby)
}

fn tree_for(target: BuildTarget) -> PathBuf {
    // Per CALL, not just per process: the tests in this file run in
    // parallel, and each clears its directory first — sharing one let a
    // test ingest a tree the other had just removed.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("roundhouse-url-options-recall-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, body) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"body\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/home_controller.rb", "class HomeController < ApplicationController\n  def top\n    @page = 1\n  end\n\n  def newest\n    @page = 1\n  end\nend\n"),
        ("app/views/home/top.html.erb", "<%= link_to \"next\", { controller: controller_name, action: action_name, page: @page + 1 } %>\n"),
        ("app/views/home/newest.html.erb", "<%= link_to \"next\", { controller: controller_name, action: action_name, page: @page + 1 } %>\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/top(/:length(/page/:page))\" => \"home#top\", :as => \"top\"\n  get \"/newest(/page/:page)\" => \"home#newest\"\nend\n"),
    ] {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let mut app = ingest_app(&dir).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = roundhouse::project::target_files(&app, &dir, target).expect("files");
    let out = dir.join("emitted");
    roundhouse::project::write_to_dir(&files, &out).expect("write");
    out
}

#[test]
fn a_view_using_the_options_hash_takes_the_request_path_parameters() {
    let out = tree();
    let view = std::fs::read_to_string(out.join("app/views/home/top.rb")).expect("home/top view");
    assert!(view.contains("path_parameters"), "{view}");
    let controller =
        std::fs::read_to_string(out.join("app/controllers/home_controller.rb")).expect("controller");
    assert!(controller.contains("@path_parameters"), "{controller}");
}

/// The view's sidecar types the param as the Hash it is: typed `String?`
/// like the other trailing extras, Spinel compiled every lobsters story
/// list to raise "no implicit conversion of Hash into String" on the call.
#[test]
fn the_spinel_sidecar_types_path_parameters_as_the_request_hash() {
    let out = tree_for(BuildTarget::Spinel);
    let rbs = std::fs::read_to_string(out.join("app/views/home/top.rbs")).expect("home/top sidecar");
    assert!(rbs.contains("?Hash[String, String]? path_parameters"), "{rbs}");
}

#[test]
fn the_resolver_recalls_a_segment_as_rails_does() {
    if !Command::new("ruby").args(["-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby not available");
        return;
    }
    let out = tree();
    let script = r#"
require File.expand_path("runtime/action_view/view_helpers", Dir.pwd)
require File.expand_path("app/route_helpers", Dir.pwd)
r = RouteHelpers
puts r.path_for_controller_action_page("home", "top", "2", { "length" => "1y" })
puts r.path_for_controller_action_page("home", "top", "1", { "length" => "1y", "page" => "2" })
puts r.path_for_controller_action_page("home", "top", "2", {})
puts r.path_for_controller_action_page("home", "newest", "2", {})
"#;
    let result = Command::new("ruby").arg("-e").arg(script).current_dir(&out).output().expect("ruby");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "/top/1y/page/2\n/top/1y/page/1\n/top\n/newest/page/2\n"
    );
}
