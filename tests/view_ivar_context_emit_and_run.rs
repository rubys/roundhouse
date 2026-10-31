#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONTROLLER_TEST: &str = "test/controllers/articles_controller_test.rb";
const INDEX_ASSERTION: &str = "assert_select \"h1\", \"Articles\"\n";

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/application_controller.rb",
            "  stale_when_importmap_changes\n",
            "  stale_when_importmap_changes\n\n  before_action :set_viewer\n\n  def set_viewer\n    @viewer = \"alice\"\n  end\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @above = {partial: \"banner\", locals: {label: \"Latest\"}}\n    raise \"never assigned, so never raised\" if @never_assigned\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<%= turbo_stream_from \"articles\" %>\n",
            "<%= turbo_stream_from \"articles\" %>\n<%= render @above if @above %>\n<%= viewer_badge(\"not-the-viewer\") %>\n<%= self.viewer_flag(\"nested\") %>\n",
        )
        .write(
            "app/views/articles/_banner.html.erb",
            "<%# locals: (label:) -%>\n<p id=\"banner\"><%= label %> for <%= @viewer.upcase %></p>\n",
        )
        .write(
            "app/views/articles/_badge.html.erb",
            "<%# locals: (label:) -%>\n<p id=\"badge\"><%= label %> / <%= @viewer.upcase %></p>\n",
        )
        .write(
            "app/views/admin/articles/_flag.html.erb",
            "<%# locals: (label:) -%>\n<p id=\"flag\"><%= label %> / <%= @viewer.upcase %></p>\n",
        )
        .edit(
            "app/helpers/articles_helper.rb",
            "module ArticlesHelper\n",
            "module ArticlesHelper\n  def viewer_badge(viewer)\n    render partial: \"articles/badge\", locals: {label: viewer}\n  end\n\n  def viewer_flag(label)\n    render partial: \"admin/articles/flag\", locals: {label: label}\n  end\n",
        )
}

#[test]
fn ivars_reached_through_dynamic_partials_helpers_and_never_assigned_check_clean() {
    let errors = overlay().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn ivars_reached_through_dynamic_partials_helpers_and_never_assigned_run() {
    overlay()
        .edit(
            CONTROLLER_TEST,
            INDEX_ASSERTION,
            &format!(
                "{INDEX_ASSERTION}    assert_includes response.body, \"<p id=\\\"banner\\\">Latest for ALICE</p>\"\n    assert_includes response.body, \"<p id=\\\"badge\\\">not-the-viewer / ALICE</p>\"\n    assert_includes response.body, \"<p id=\\\"flag\\\">nested / ALICE</p>\"\n"
            ),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}
