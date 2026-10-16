//! campfire's SQLite-observer caches compiled natively; the shared
//! contract and its CRuby twin are described in
//! `tests/support/campfire_caches.rs`.

use super::campfire_caches_contract::{self as contract, Contract};

fn assert_runs_natively(contract: &Contract) {
    let run = contract.overlay().run_spinel(contract.script);
    run.assert_passes();
    assert_eq!(run.stdout, contract.expected, "stderr:\n{}", run.stderr);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn nested_multi_write_destructures_each_group_natively() {
    assert_runs_natively(&contract::NESTED_MULTI_WRITE);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn data_define_block_methods_belong_to_the_data_class_natively() {
    let source = contract::DATA_BLOCK_METHODS
        .source
        .replace("DataKeySupportController.encode(digest)", "\"encoded-\" + digest");
    let run = contract::DATA_BLOCK_METHODS
        .overlay()
        .write(contract::DATA_BLOCK_METHODS.path, &source)
        .run_spinel(contract::DATA_BLOCK_METHODS.script);
    run.assert_passes();
    assert_eq!(run.stdout, contract::DATA_BLOCK_METHODS.expected, "stderr:\n{}", run.stderr);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn response_helpers_run_as_rails_runs_them_natively() {
    assert_runs_natively(&contract::RESPONSE_HELPERS);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn dispatched_request_headers_are_visible_to_headers_api_natively() {
    use super::native_http;

    let (tree, errors) = super::emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/request-header\", to: \"request_headers#show\"\n",
        )
        .write(
            "app/controllers/request_headers_controller.rb",
            r#"class RequestHeadersController < ApplicationController
  def show
    render plain: [
      request.headers["Accept-Encoding"].to_s,
      request.headers["If-None-Match"].to_s,
      request.headers["If-Modified-Since"].to_s,
      request.headers["Turbo-Frame"].to_s
    ].join("|")
  end
end
"#,
        )
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    let headers = [
        ("Accept-Encoding", "br, gzip"),
        ("If-None-Match", "W/\"etag\""),
        ("If-Modified-Since", "Sat, 10 Oct 2026 00:00:00 GMT"),
        ("Turbo-Frame", "room_messages"),
    ];
    let response = server.get_with_headers("/request-header", &headers);
    assert_eq!(response.status, 200, "{}\n{}", response.body, server.log());
    assert_eq!(
        response.body,
        "br, gzip|W/\"etag\"|Sat, 10 Oct 2026 00:00:00 GMT|room_messages"
    );
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn sqlite_observer_and_checkpointer_surface_runs_natively() {
    assert_runs_natively(&contract::SQLITE_OBSERVER);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn record_snapshots_and_the_bounded_store_run_natively() {
    assert_runs_natively(&contract::RECORD_SNAPSHOT);
}

/// The native half of `emit_and_run::view_fragments_go_through_the_
/// controllers_caching`, over HTTP: the production server caches (Rails'
/// default), a commit that does not touch `updated_at` is served stale,
/// `?nocache=1` (the app's `perform_caching` override) renders fresh, and
/// moving the app's epoch (its `combined_fragment_cache_key` override)
/// misses into the new title.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn view_fragments_go_through_the_controllers_caching_natively() {
    use super::native_http;
    const FORM: &str = "application/x-www-form-urlencoded";
    let (tree, errors) = contract::fragments_overlay().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let mut server = native_http::Server::start(&tree);
    server.take_session("/articles/new");
    let created = server.post(
        "/articles.json",
        FORM,
        "article%5Btitle%5D=Original&article%5Bbody%5D=A+sufficiently+long+article+body.",
    );
    assert_eq!(created.status, 201, "{}\n{}", created.body, server.log());
    let page = || server.get("/fragments/1").body;
    assert!(page().contains("Original"), "{}", server.log());
    let foreign = std::process::Command::new("sqlite3")
        .arg(tree.join("native_http.sqlite3"))
        .arg("UPDATE articles SET title = 'Changed' WHERE id = 1")
        .output()
        .expect("run sqlite3");
    assert!(foreign.status.success(), "{}", String::from_utf8_lossy(&foreign.stderr));
    assert!(page().contains("Original"), "the cached fragment is served:\n{}", server.log());
    assert!(server.get("/fragments/1?nocache=1").body.contains("Changed"), "{}", server.log());
    assert_eq!(server.post("/fragments/bump", FORM, "").status, 200, "{}", server.log());
    assert!(page().contains("Changed"), "the moved epoch misses:\n{}", server.log());
}

/// The native half of `qr_code_capacity_error_is_rescued_by_name`, over
/// HTTP against the spinel-rqrcode package.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn qr_code_capacity_error_is_rescued_by_name_natively() {
    use super::native_http;
    let (tree, errors) = contract::qr_code_overlay().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    let short = server.get("/qr/10");
    assert_eq!(short.status, 200, "{}\n{}", short.body, server.log());
    assert!(short.body.contains("<svg"), "{}", short.body);
    let long = server.get("/qr/8000");
    assert_eq!(long.status, 400, "{}\n{}", long.body, server.log());
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_cache_through_method_answers_its_blocks_value_on_hit_and_miss_natively() {
    assert_runs_natively(&contract::CACHE_THROUGH);
}
