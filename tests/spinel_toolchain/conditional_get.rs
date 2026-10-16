//! Rack::ETag and Rack::ConditionalGet over the native HTTP server; the
//! Ruby-lane twin is `emit_and_run/conditional_get.rs`.

use super::conditional_get_contract as contract;
use super::native_http;
use roundhouse::project::BuildTarget;

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn responses_carry_etag_cache_control_and_answer_304_natively() {
    let (tree, errors) = contract::overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors:\n{}", errors.join("\n"));
    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    let header = |r: &native_http::Response, name: &str| r.headers.get(name).cloned();

    let index = server.get("/articles");
    assert_eq!(index.status, 200, "{}", server.log());
    let etag = header(&index, "etag").unwrap_or_default();
    assert!(
        etag.len() == 36 && etag.starts_with("W/\"") && etag[3..35].chars().all(|c| c.is_ascii_hexdigit()),
        "index ETag {etag:?}"
    );
    assert_eq!(header(&index, "cache-control").as_deref(), Some(contract::DEFAULT_CACHE_CONTROL));

    let stable = server.get("/etag-probe/stable");
    assert_eq!(stable.status, 200, "{}", server.log());
    assert_eq!(header(&stable, "etag").as_deref(), Some(contract::STABLE_ETAG));
    assert_eq!(header(&stable, "cache-control").as_deref(), Some(contract::DEFAULT_CACHE_CONTROL));

    let fresh = server.get_with_headers("/etag-probe/stable", &[("If-None-Match", contract::STABLE_ETAG)]);
    assert_eq!(fresh.status, 304, "{}", server.log());
    assert!(fresh.body.is_empty(), "304 body {:?}", fresh.body);
    assert!(!fresh.headers.contains_key("content-type"), "{:?}", fresh.headers);
    assert_eq!(header(&fresh, "etag").as_deref(), Some(contract::STABLE_ETAG));

    let strong = contract::STABLE_ETAG.trim_start_matches("W/");
    assert_eq!(server.get_with_headers("/etag-probe/stable", &[("If-None-Match", strong)]).status, 200);

    let away = server.get("/etag-probe/away");
    assert_eq!(away.status, 302, "{}", server.log());
    assert!(!away.headers.contains_key("etag"), "{:?}", away.headers);
    assert_eq!(header(&away, "cache-control").as_deref(), Some("no-cache"));
}
