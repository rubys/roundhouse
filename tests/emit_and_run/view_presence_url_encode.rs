//! Two helpers a view reaches through Rails' framework: `x.presence`
//! (ActiveSupport's Object#presence) in an output tag, and `u(x)` /
//! `url_encode(x)` (ERB::Util, which Rails includes in every view).

use roundhouse::project::BuildTarget;

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/probes\", to: \"probes#show\"\n",
        )
        .write(
            "app/controllers/probes_controller.rb",
            r#"class ProbesController < ApplicationController
  def show
    @query = "a b/ü&~x"
    @empty = ""
  end
end
"#,
        )
        .write(
            "app/views/probes/show.html.erb",
            "<p id=\"u\"><%= u(@query) %></p>\n<p id=\"url_encode\"><%= url_encode(@query) %></p>\n\
             <p id=\"empty\"><%= @empty.presence || \"fallback\" %></p>\n<p id=\"query\"><%= @query.presence %></p>\n",
        )
        .write(
            "test/controllers/probes_controller_test.rb",
            r#"require "test_helper"

class ProbesControllerTest < ActionDispatch::IntegrationTest
  test "presence and url_encode answer as in Rails" do
    get "/probes"
    assert_response :success
    assert_includes response.body, "<p id=\"u\">a%20b%2F%C3%BC%26~x</p>"
    assert_includes response.body, "<p id=\"url_encode\">a%20b%2F%C3%BC%26~x</p>"
    assert_includes response.body, "<p id=\"empty\">fallback</p>"
    assert_includes response.body, "<p id=\"query\">a b/ü&amp;~x</p>"
  end
end
"#,
        )
}

#[test]
fn view_presence_and_url_encode_render_as_in_rails_on_ruby() {
    app().run_test("test/controllers/probes_controller_test.rb").assert_passes();
}

/// The Spinel tree has no Object#presence to reopen onto every value, so
/// the view's call is lowered to the runtime predicate, and `u` to the
/// runtime's ERB::Util.url_encode.
#[test]
fn view_presence_and_url_encode_lower_to_the_runtime_on_spinel() {
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let view = std::fs::read_dir(emitted.join("app/views/probes"))
        .expect("app/views/probes")
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(view.contains("ActiveSupport.presence(empty)"), "{view}");
    assert!(view.contains("erb_url_encode("), "{view}");
    assert!(!view.contains("empty.presence") && !view.contains("query.presence"), "{view}");
}
