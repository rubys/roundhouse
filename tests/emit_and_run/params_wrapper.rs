//! Rails' ParamsWrapper on the emitted app: a JSON body is wrapped under the
//! controller's model name by default, and `wrap_parameters false` turns it off.

use super::emit_and_run;

/// A JSON POST through the emitted app's own request parsing
/// (`Main.dispatch_core`, the path Puma serves), answering
/// `[status, created title or nil]`.
const POST_JSON: &str = r#"
require "json"
require "stringio"
# CSRF off, as Rails' test environment has it (the Rails measurements
# were taken that way); this is about the body, not the token.
ActionController::Base.allow_forgery_protection = false
def post_json(path, payload)
  body = JSON.generate(payload)
  env = {
    "REQUEST_METHOD" => "POST", "PATH_INFO" => path, "QUERY_STRING" => "",
    "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s,
    "HTTP_ACCEPT" => "application/json"
  }
  status, _body = Main.dispatch_core(env, StringIO.new(body))
  status
end
"#;

/// Rails wraps a JSON body under the controller's model name by default
/// (`wrap_parameters_by_default`, on since `load_defaults 7.0`; the
/// fixture is on 8.1): a client posting `{"title":…,"body":…}` to the
/// scaffold's `create`, whose `params.expect(article: …)` reads
/// `params[:article]`, gets the article created. Measured on the fixture
/// itself under Rails 8.1.4: 201.
#[test]
fn a_json_body_is_wrapped_under_the_model_name_by_default() {
    emit_and_run::real_blog()
        .run_ruby(&format!(
            r#"{POST_JSON}
status = post_json("/articles.json", {{ "title" => "Wrapped by default", "body" => "A body long enough" }})
raise "expected 201 as Rails answers, got #{{status.inspect}}" unless status == 201
raise "the article was not created" unless Article.find_by(title: "Wrapped by default")
puts "default wrapping OK"
"#
        ))
        .assert_passes();
}

/// `wrap_parameters false` switches that off: the same top-level body
/// has no `article` key, so `params.expect(article: …)` refuses it — 400
/// in Rails 8.1.4 on the fixture — while a client that nests the object
/// itself still creates the article (201).
#[test]
fn wrap_parameters_false_leaves_a_json_body_unwrapped() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  wrap_parameters false\n",
        )
        .run_ruby(&format!(
            r#"{POST_JSON}
status = post_json("/articles.json", {{ "title" => "Not wrapped", "body" => "A body long enough" }})
raise "expected 400 as Rails answers, got #{{status.inspect}}" unless status == 400
status = post_json("/articles.json", {{ "article" => {{ "title" => "Nested by client", "body" => "A body long enough" }} }})
raise "expected 201 as Rails answers, got #{{status.inspect}}" unless status == 201
raise "the article was not created" unless Article.find_by(title: "Nested by client")
puts "wrap_parameters false OK"
"#
        ))
        .assert_passes();
}

/// Wrapping a JSON body that carries nothing to wrap still sets the key:
/// Rails 8.1 answers `params` = `{"article" => {}}` for an empty body or
/// `{}` sent as JSON, and for `{"foo":1}` when the model has no `foo`
/// attribute. `params.require(:article)` then raises `ParameterMissing`
/// (400) because Rails' `require` refuses a BLANK value, and `{}` is
/// blank. A controller whose only guard is a standalone `require` must
/// not go on to create anything. Measured on Rails 8.1.3 with a
/// `load_defaults 8.1` `ActionController::API` app: 400 for every body
/// below except the nested one, which passes (201 here).
#[test]
fn a_wrapped_empty_body_does_not_satisfy_a_standalone_require() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            "    params.require(:article)\n    @article = Article.new(title: \"Guarded only by require\", body: \"A body long enough\")\n",
        )
        .run_ruby(&format!(
            r#"{POST_JSON}
def post_raw(path, body)
  env = {{
    "REQUEST_METHOD" => "POST", "PATH_INFO" => path, "QUERY_STRING" => "",
    "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s,
    "HTTP_ACCEPT" => "application/json"
  }}
  status, _body = Main.dispatch_core(env, StringIO.new(body))
  status
end
results = {{
  "empty body" => post_raw("/articles.json", ""),
  "{{}}" => post_json("/articles.json", {{}}),
  "unrelated key" => post_json("/articles.json", {{ "foo" => 1 }}),
  "article: {{}}" => post_json("/articles.json", {{ "article" => {{}} }}),
}}
created = Article.where(title: "Guarded only by require").count
bad = results.reject {{ |_k, s| s == 400 }}
raise "expected 400 for each, as Rails answers; got #{{results.inspect}} (#{{created}} rows written)" unless bad.empty?
raise "a refused request wrote #{{created}} rows" unless created == 0
status = post_json("/articles.json", {{ "article" => {{ "title" => "x" }} }})
raise "expected 201 for a nested article, got #{{status.inspect}}" unless status == 201
puts "standalone require OK"
"#
        ))
        .assert_passes();
}
