//! An inlined `before_action`'s `if:`/`unless:` guard must still gate
//! the filter at request time.
//!
//! `inline_before_filters` used to splice a filter target's private
//! method body straight into the action, regardless of any `if:` /
//! `unless:` on the `before_action` — so `before_action :reject_paging,
//! if: :paging?` answered 401 for every request, paged or not. A
//! guarded filter now runs from the dispatcher preamble instead, under
//! the same condition Rails would check.

use super::emit_and_run;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/widgets\", to: \"widgets#index\"\n  get \"/gizmos\", to: \"gizmos#index\"\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  before_action :reject_paging, if: :paging?

  def index
    head :ok
  end

  private

  def paging?
    params[:page].present?
  end

  def reject_paging
    head :unauthorized
  end
end
"#,
        )
        .write(
            "app/controllers/gizmos_controller.rb",
            r#"class GizmosController < ApplicationController
  before_action :reject_anonymous, unless: :signed_in?

  def index
    head :ok
  end

  private

  def signed_in?
    params[:token].present?
  end

  def reject_anonymous
    head :unauthorized
  end
end
"#,
        )
}

const ASSERTIONS: &str = r#"
status, = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets", "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
)
raise "GET /widgets (no page) answered #{status}, want 200" unless status == 200

status, = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets", "QUERY_STRING" => "page=2",
  "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
)
raise "GET /widgets?page=2 answered #{status}, want 401" unless status == 401

status, = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/gizmos", "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
)
raise "GET /gizmos (signed out) answered #{status}, want 401" unless status == 401

status, = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/gizmos", "QUERY_STRING" => "token=abc",
  "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
)
raise "GET /gizmos?token=abc answered #{status}, want 200" unless status == 200
puts "inline filter guard passed"
"#;

#[test]
fn an_inlined_before_actions_guard_is_evaluated_at_request_time() {
    let run = overlay().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("inline filter guard passed"));
}
