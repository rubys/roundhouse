//! Strong parameters refuse a request natively the way Rails does: the
//! blog built with `spin build` and driven over HTTP (see
//! `tests/support/native_http.rs`), against the statuses Rails 8.1.4
//! answers on the same fixture.

use super::{emit_and_run, native_http};

/// The native half of `emit_and_run::a_missing_resource_key_answers_400`:
/// real-blog's `params.expect(article: [:title, :body])` refuses a
/// request without a usable `article` hash, and the server answers 400
/// as Rails does. Statuses measured on the same fixture under Rails 8.1.4.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn strong_params_refusals_answer_like_rails_natively() {
    const FORM: &str = "application/x-www-form-urlencoded";
    let (tree, errors) = emit_and_run::real_blog().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let mut server = native_http::Server::start(&tree);
    server.take_session("/articles/new");
    let cases = [
        ("no article key", FORM, "title=Flat&body=A+body+long+enough", 400),
        ("article is a scalar", FORM, "article=oops", 400),
        ("article nested", FORM, "article%5Btitle%5D=Nested&article%5Bbody%5D=A+body+long+enough", 201),
    ];
    for (name, content_type, body, status) in cases {
        let response = server.post("/articles.json", content_type, body);
        assert_eq!(response.status, status, "{name}: {}\n{}", response.body, server.log());
    }
    // The token is what lets the requests above through.
    let response = server.post_without_session("/articles.json", FORM, cases[2].2);
    assert_eq!(response.status, 422, "without a CSRF token: {}", response.body);
}

/// The native half of `emit_and_run::require_permit_refuses_like_rails`:
/// with the helper switched to `params.require(:article).permit(...)`,
/// a blank resource answers 400 and a non-hash one 500, as in Rails.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn require_permit_refusals_answer_like_rails_natively() {
    const FORM: &str = "application/x-www-form-urlencoded";
    let (tree, errors) = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "params.expect(article: [ :title, :body ])",
            "params.require(:article).permit(:title, :body)",
        )
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let mut server = native_http::Server::start(&tree);
    server.take_session("/articles/new");
    let cases = [
        ("no article key", FORM, "title=Flat&body=A+body+long+enough", 400),
        ("article is whitespace", FORM, "article=%20%20", 400),
        ("article is an empty array", "application/json", r#"{"article":[]}"#, 400),
        ("article is a scalar", FORM, "article=oops", 500),
        ("article nested", FORM, "article%5Btitle%5D=Nested&article%5Bbody%5D=A+body+long+enough", 201),
    ];
    for (name, content_type, body, status) in cases {
        let response = server.post("/articles.json", content_type, body);
        assert_eq!(response.status, status, "{name}: {}\n{}", response.body, server.log());
    }
}
