//! #1593: `current_page?(path)`. Rails 8.1.4 answers it against the
//! request: a GET or HEAD, both sides percent-decoded, the request's
//! fullpath only when the path has a query, a trailing `/` dropped.

use super::emit_and_run;

/// real-blog's index and show pages, each asking whether it is the index.
fn app() -> emit_and_run::Overlay {
    let nav = concat!(
        "<p id=\"index\"><%= current_page?(articles_path) %></p>\n",
        "<p id=\"slash\"><%= current_page?(\"/articles/\") %></p>\n",
        "<p id=\"query\"><%= current_page?(\"/articles?order=asc\") %></p>\n",
        "<p id=\"url\"><%= current_page?(articles_url) %></p>\n",
        "<p id=\"encoded-url\"><%= current_page?(\"http%3A%2F%2Fwww.example.com/articles\") %></p>\n",
        "<p id=\"other-host\"><%= current_page?(\"http://other.example/articles\") %></p>\n",
    );
    emit_and_run::real_blog()
        .edit("app/views/articles/index.html.erb", "<%= turbo_stream_from \"articles\" %>\n", &format!("<%= turbo_stream_from \"articles\" %>\n{nav}"))
        .edit("app/views/articles/show.html.erb", "<% content_for :title, \"Showing article\" %>\n", &format!("<% content_for :title, \"Showing article\" %>\n{nav}"))
}

const TEST: &str = r#"require "test_helper"

class CurrentPageControllerTest < ActionDispatch::IntegrationTest
  test "the index is the current page on the index" do
    get articles_url
    assert_response :success
    assert_select "p#index", "true"
    assert_select "p#slash", "true"
    assert_select "p#query", "false"
    assert_select "p#url", "true"
    assert_select "p#encoded-url", "true"
    assert_select "p#other-host", "false"
  end

  test "a query on the request is ignored unless the path has one" do
    get articles_url(order: "asc")
    assert_response :success
    assert_select "p#index", "true"
    assert_select "p#query", "true"
  end

  test "the index is not the current page on show" do
    get article_url(articles(:one))
    assert_response :success
    assert_select "p#index", "false"
    assert_select "p#slash", "false"
    assert_select "p#url", "false"
    assert_select "p#encoded-url", "false"
  end
end
"#;

#[test]
fn current_page_answers_as_rails_does() {
    app()
        .write("test/controllers/current_page_controller_test.rb", TEST)
        .run_test("test/controllers/current_page_controller_test.rb")
        .assert_passes();
}

/// The `url_for` options form and `check_parameters:`/`method:` are not
/// modeled.
#[test]
fn a_current_page_form_rails_answers_through_url_for_is_refused() {
    for call in [
        "current_page?(controller: \"articles\", action: \"index\")",
        "current_page?(articles_path, check_parameters: true)",
    ] {
        let errors = emit_and_run::real_blog()
            .edit("app/views/articles/index.html.erb", "<%= turbo_stream_from \"articles\" %>\n", &format!("<%= turbo_stream_from \"articles\" %>\n<%= {call} %>\n"))
            .check_errors();
        assert!(errors.iter().any(|e| e.contains("current_page? not supported")), "{call}: {errors:?}");
    }
}

/// The index and show pages in the spinel binary, where #1593 was found.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn current_page_answers_as_rails_does_on_spinel() {
    app()
        .run_spinel(concat!(
            "Db.configure(\":memory:\")\n",
            "Schema.statements.each { |sql| Db.exec(sql) }\n",
            "ActiveRecord.adapter = SqliteAdapter\n",
            "Db.exec(\"INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (1, 'Title', 'Body', '2026-11-01 09:30:15', '2026-11-01 09:30:15')\")\n",
            "def page(action, path, query, params)\n",
            "  request = ActionDispatch::Request.new\n",
            "  request.request_method = \"GET\"\n",
            "  request.path = path\n",
            "  request.query_string = query\n",
            "  ActionController::Current.request = request\n",
            "  controller = ArticlesController.new\n",
            "  controller.request_method = \"GET\"\n",
            "  controller.params = params\n",
            "  controller.process_action(action)\n",
            "  controller.body\n",
            "end\n",
            "index = page(:index, \"/articles\", \"\", {})\n",
            "raise index unless index.include?('<p id=\"index\">true</p>') && index.include?('<p id=\"slash\">true</p>') && index.include?('<p id=\"query\">false</p>')\n",
            "sorted = page(:index, \"/articles\", \"order=asc\", {})\n",
            "raise sorted unless sorted.include?('<p id=\"index\">true</p>') && sorted.include?('<p id=\"query\">true</p>')\n",
            "show = page(:show, \"/articles/1\", \"\", {\"id\" => \"1\"})\n",
            "raise show unless show.include?('<p id=\"index\">false</p>')\n",
            "puts \"current_page passed\"\n",
        ))
        .assert_passes();
}

/// Off the ruby family a view has no request to read (TypeScript:
/// `request is not defined`; Crystal, Python and Rust likewise), and Roda
/// has no ActionView, so `current_page?` is refused there, naming the
/// target.
#[test]
fn current_page_is_refused_where_views_have_no_request() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Typescript, BuildTarget::Crystal, BuildTarget::Go, BuildTarget::Python, BuildTarget::Rust, BuildTarget::Roda] {
        let (_, errors) = app().emit(target);
        assert!(errors.iter().any(|e| e.contains("current_page? not supported")), "{target:?}: {errors:?}");
    }
}

/// An app's own `current_page?` (here an `ApplicationHelper` method) is
/// what Rails calls, ahead of ActionView's: neither rewritten nor refused.
#[test]
fn an_app_defined_current_page_is_left_to_the_app() {
    let errors = emit_and_run::real_blog()
        .write("app/helpers/application_helper.rb", "module ApplicationHelper\n  def current_page?(path, check_parameters: false)\n    path == \"/articles\"\n  end\nend\n")
        .edit("app/views/articles/index.html.erb", "<%= turbo_stream_from \"articles\" %>\n", "<%= turbo_stream_from \"articles\" %>\n<%= current_page?(articles_path, check_parameters: true) %>\n")
        .check_errors();
    assert!(!errors.iter().any(|e| e.contains("current_page?")), "{errors:?}");
}

/// A splat may be anything, the options form included: left as written,
/// and refused.
#[test]
fn a_current_page_with_a_splat_is_refused() {
    let errors = emit_and_run::real_blog()
        .edit("app/views/articles/index.html.erb", "<%= turbo_stream_from \"articles\" %>\n", "<%= turbo_stream_from \"articles\" %>\n<% args = [articles_path] %><%= current_page?(*args) %>\n")
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("current_page? not supported")), "{errors:?}");
}

/// Only views are rewritten: a helper module's `current_page?` is refused
/// (its `ActionView::ViewHelpers` call wouldn't resolve there), and a plain
/// class's own `current_page?` is its own, neither rewritten nor refused.
#[test]
fn only_views_are_rewritten() {
    let helper = emit_and_run::real_blog()
        .write("app/helpers/nav_helper.rb", "module NavHelper\n  def on_articles?\n    current_page?(\"/articles\")\n  end\nend\n")
        .check_errors();
    assert!(helper.iter().any(|e| e.contains("current_page? not supported")), "{helper:?}");
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/models/pager.rb", "class Pager\n  def current_page?(path)\n    path == \"/\"\n  end\n\n  def home?\n    current_page?(\"/\")\n  end\nend\n")
        .edit("app/views/articles/index.html.erb", "<%= turbo_stream_from \"articles\" %>\n", "<%= turbo_stream_from \"articles\" %>\n<p id=\"pager\"><%= Pager.new.home? %></p>\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let pager = std::fs::read_to_string(emitted.join("app/models/pager.rb")).unwrap();
    assert!(pager.contains("current_page?(\"/\")") && !pager.contains("current_page_match?"), "{pager}");
}

/// A template no request reaches keeps its errors as warnings, as analysis
/// does, Jbuilder's included: a leading `return if …` guard gives its body
/// a synthetic span, so reachability goes by the view's name.
#[test]
fn current_page_in_an_unreachable_template_is_a_warning() {
    use roundhouse::project::BuildTarget;
    let orphan = "return if @skip\njson.here current_page?(\"/articles\")\njson.title \"orphan\"\n";
    let overlay = || emit_and_run::real_blog().write("app/views/articles/orphan.json.jbuilder", orphan);
    let (_, errors) = overlay().emit(BuildTarget::Typescript);
    assert!(!errors.iter().any(|e| e.contains("current_page?")), "{errors:?}");
    let (_, errors) = overlay()
        .edit("config/routes.rb", "Rails.application.routes.draw do\n", "Rails.application.routes.draw do\n  get \"orphan\", to: \"articles#orphan\"\n")
        .emit(BuildTarget::Typescript);
    assert!(errors.iter().any(|e| e.contains("current_page? not supported")), "a routed template keeps the error: {errors:?}");
}

/// The rewrite runs before types: an argument analysis can't type as a
/// String (a record, which Rails passes to `url_for`; an options Hash in
/// a variable; a value that may be nil) is refused, and route helpers,
/// literals and interpolated paths stay rewritten.
#[test]
fn a_current_page_argument_not_typed_as_a_string_is_refused() {
    for call in [
        "current_page?(@article)",
        "current_page?(Article.first)",
        "current_page?(@article.title.presence)",
    ] {
        let errors = emit_and_run::real_blog()
            .edit("app/views/articles/show.html.erb", "<% content_for :title, \"Showing article\" %>\n", &format!("<% content_for :title, \"Showing article\" %>\n<%= {call} %>\n"))
            .check_errors();
        assert!(errors.iter().any(|e| e.contains("current_page? not supported") && e.contains("String")), "{call}: {errors:?}");
    }
    for call in [
        "current_page?(articles_path)",
        "current_page?(article_url(@article))",
        "current_page?(\"/articles\")",
        "current_page?(\"/articles/#{@article.id}\")",
    ] {
        let errors = emit_and_run::real_blog()
            .edit("app/views/articles/show.html.erb", "<% content_for :title, \"Showing article\" %>\n", &format!("<% content_for :title, \"Showing article\" %>\n<%= {call} %>\n"))
            .check_errors();
        assert!(!errors.iter().any(|e| e.contains("current_page?")), "{call}: {errors:?}");
    }
}

/// The runtime helper the rewrite calls takes the request's parts as
/// Strings. It is emitted with the shared runtime on every target, and
/// these have no request type for a parameter to name: on 1f7957428 the
/// Rust tree's `current_page_path_pred(request: Request, …)` failed with
/// `cannot find type Request`, in an app with no `current_page?`.
#[test]
fn the_runtime_helper_names_no_request_type() {
    use roundhouse::project::BuildTarget;
    fn visit(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, out);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                out.extend(
                    text.lines()
                        .filter(|l| l.to_lowercase().replace('_', "").contains("currentpage") && l.contains("Request"))
                        .map(|l| format!("{}: {}", path.display(), l.trim())),
                );
            }
        }
    }
    for target in [BuildTarget::Rust, BuildTarget::Go, BuildTarget::Kotlin, BuildTarget::Swift, BuildTarget::CSharp] {
        let (emitted, _) = emit_and_run::real_blog().emit(target);
        let mut found = Vec::new();
        visit(&emitted, &mut found);
        assert!(found.is_empty(), "{target:?}: {found:#?}");
    }
}
