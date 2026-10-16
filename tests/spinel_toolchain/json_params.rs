//! A JSON body that does not parse, on the native lane over HTTP (see
//! `tests/support/native_http.rs`): Rails answers it 400 before any
//! action runs (see `tests/support/json_params.rs`). The well-formed
//! bodies' values are covered by `json_request_values.rs`.

use super::{json_params_contract as contract, native_http};

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_malformed_json_body_is_answered_400_natively() {
    let (tree, errors) = contract::blog().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let mut server = native_http::Server::start(&tree);
    server.take_session("/articles/new");
    let (body, expected) = contract::CASES[0];
    let response = server.post("/echo", "application/json", body);
    assert_eq!(response.status, 200, "{body}: {}\n{}", response.body, server.log());
    assert_eq!(response.body, expected, "{body}\n{}", server.log());
    let response = server.post("/echo", "application/json", contract::MALFORMED);
    assert_eq!(response.status, 400, "malformed JSON: {}\n{}", response.body, server.log());
}
