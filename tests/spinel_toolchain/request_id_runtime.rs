//! X-Request-Id and X-Runtime over the native HTTP server; the
//! Ruby-lane twin is `emit_and_run/request_id_runtime.rs`.

use super::native_http;
use super::request_id_runtime_contract as contract;
use roundhouse::project::BuildTarget;

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn responses_carry_request_id_and_runtime_natively() {
    let (tree, errors) = contract::overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors:\n{}", errors.join("\n"));
    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    let header = |r: &native_http::Response, name: &str| r.headers.get(name).cloned().unwrap_or_default();

    let page = server.get("/articles");
    assert_eq!(page.status, 200, "{}", server.log());
    assert!(contract::is_uuid(&header(&page, "x-request-id")), "{:?}", page.headers);
    assert!(contract::is_runtime(&header(&page, "x-runtime")), "{:?}", page.headers);
    let again = server.get("/articles");
    assert_ne!(header(&again, "x-request-id"), header(&page, "x-request-id"));

    let client = server.get_with_headers("/articles", &[("X-Request-Id", contract::CLIENT_ID)]);
    assert_eq!(header(&client, "x-request-id"), contract::CLEANED_ID);

    let missing = server.get("/no-such-page");
    assert_eq!(missing.status, 404, "{}", server.log());
    assert!(contract::is_uuid(&header(&missing, "x-request-id")), "{:?}", missing.headers);
    assert!(contract::is_runtime(&header(&missing, "x-runtime")), "{:?}", missing.headers);

    // ActionDispatch::Static answers ahead of both middleware.
    let file = server.get("/robots.txt");
    assert_eq!(file.status, 200, "{}", server.log());
    assert!(!file.headers.contains_key("x-request-id"), "{:?}", file.headers);
    assert!(!file.headers.contains_key("x-runtime"), "{:?}", file.headers);
}
