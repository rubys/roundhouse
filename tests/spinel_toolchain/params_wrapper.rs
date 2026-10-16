//! The native half of `emit_and_run::params_wrapper`: the blog built with
//! `spin build` wraps a JSON body as Rails does, over HTTP.

use super::{emit_and_run, native_http};

/// The native half of `emit_and_run::a_json_body_is_wrapped_under_the_model_name_by_default`:
/// real-blog loads Rails 8.1 defaults, so a JSON body posted at the top
/// level is wrapped under `article` before `params.expect` reads it, as
/// Rails' ParamsWrapper does. Measured on the same fixture under Rails 8.1.4.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_json_body_is_wrapped_natively() {
    const JSON: &str = "application/json";
    let (tree, errors) = emit_and_run::real_blog().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let mut server = native_http::Server::start(&tree);
    server.take_session("/articles/new");
    let cases = [
        ("JSON at the top level", JSON, r#"{"title":"Wrapped","body":"A body long enough"}"#, 201),
        ("JSON nested by the client", JSON, r#"{"article":{"title":"Nested","body":"A body long enough"}}"#, 201),
        // Only a JSON body is wrapped.
        ("form at the top level", "application/x-www-form-urlencoded", "title=Flat&body=A+body+long+enough", 400),
    ];
    for (name, content_type, body, status) in cases {
        let response = server.post("/articles.json", content_type, body);
        assert_eq!(response.status, status, "{name}: {}\n{}", response.body, server.log());
    }
    let created = server.post("/articles.json", JSON, r#"{"title":"Echoed","body":"A body long enough"}"#);
    assert!(created.body.contains(r#""title":"Echoed""#), "{}", created.body);
}


/// The native half of
/// `emit_and_run::a_wrapped_empty_body_does_not_satisfy_a_standalone_require`:
/// a JSON body with nothing to wrap is wrapped as `article => {}`, and a
/// standalone `params.require(:article)` refuses that blank value with
/// 400, as Rails 8.1 does, instead of letting `create` run.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_wrapped_empty_body_is_refused_natively() {
    const JSON: &str = "application/json";
    let (tree, errors) = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            "    params.require(:article)\n    @article = Article.new(title: \"Guarded only by require\", body: \"A body long enough\")\n",
        )
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let mut server = native_http::Server::start(&tree);
    server.take_session("/articles/new");
    for (name, body) in [("empty body", ""), ("{}", "{}"), ("article: {}", r#"{"article":{}}"#)] {
        let response = server.post("/articles.json", JSON, body);
        assert_eq!(response.status, 400, "{name}: {}\n{}", response.body, server.log());
    }
    let nested = server.post("/articles.json", JSON, r#"{"article":{"title":"x"}}"#);
    assert_eq!(nested.status, 201, "{}\n{}", nested.body, server.log());
}
