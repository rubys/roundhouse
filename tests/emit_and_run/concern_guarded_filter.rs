//! A concern's `included` filter behind `if respond_to?(:before_action)`
//! still reaches every including controller (#1554, the first half of
//! #1373).
//!
//! Rails evaluates the `included` block on the including controller
//! class, where `respond_to?(:before_action)` is true, so the guarded
//! `before_action` registers exactly like an unguarded one. The ingest
//! handed the `IfNode` to `parse_filter_call`, which only reads calls,
//! and the filter vanished with no gap: the action answered 200
//! unauthenticated. A guard ingest cannot decide is now a survey gap
//! instead of a silent drop.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};

use super::emit_and_run;

fn concern(name: &str, included_body: &str) -> String {
    format!(
        r#"module {name}
  extend ActiveSupport::Concern

  included do
{included_body}
  end

  def require_login!
    head :unauthorized
  end

  def needs_login?
    params[:token].blank?
  end
end
"#
    )
}

fn controller(name: &str, concern: &str) -> String {
    format!(
        "class {name} < ApplicationController\n  include {concern}\n\n  def index\n    render plain: \"ok\"\n  end\nend\n"
    )
}

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/widgets\", to: \"widgets#index\"\n  get \"/gizmos\", to: \"gizmos#index\"\n",
        )
        .write(
            "app/controllers/concerns/modifier_authable.rb",
            &concern(
                "ModifierAuthable",
                "    before_action :require_login!, if: :needs_login? if respond_to?(:before_action)",
            ),
        )
        .write(
            "app/controllers/concerns/block_authable.rb",
            &concern(
                "BlockAuthable",
                "    if respond_to?(:before_action)\n      before_action :require_login!, if: :needs_login?\n    end",
            ),
        )
        .write("app/controllers/widgets_controller.rb", &controller("WidgetsController", "ModifierAuthable"))
        .write("app/controllers/gizmos_controller.rb", &controller("GizmosController", "BlockAuthable"))
}

const ASSERTIONS: &str = r#"
def get(path, query)
  status, _, body = Main.run_rack(
    "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query,
    "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
  )
  [status, body.join]
end

["/widgets", "/gizmos"].each do |path|
  status, body = get(path, "")
  raise "GET #{path} (signed out) answered #{status} #{body.inspect}, want 401" unless status == 401
  status, body = get(path, "token=abc")
  raise "GET #{path}?token=abc answered #{status}, want 200" unless status == 200
  raise "GET #{path}?token=abc body #{body.inspect}" unless body == "ok"
end
puts "guarded concern filters ran"
"#;

#[test]
fn a_concern_filter_guarded_by_respond_to_runs_in_the_including_controller() {
    let run = overlay().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("guarded concern filters ran"), "{}", run.stdout);
}

#[test]
fn a_concern_filter_behind_an_undecided_guard_is_a_survey_gap() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "app/controllers/concerns/authable.rb",
            concern("Authable", "    before_action :require_login! if ENV[\"LOCKED\"]"),
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/controllers/widgets_controller.rb", controller("WidgetsController", "Authable")),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect();
    survey::activate();
    let result = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    result.expect("an undecided guard is a survey gap, not a hard failure");
    assert!(
        gaps.iter().any(|g| matches!(
            g,
            IngestError::Unsupported { message, .. } if message.contains("before_action")
        )),
        "{gaps:?}"
    );
}
