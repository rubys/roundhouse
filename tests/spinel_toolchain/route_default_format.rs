//! The native half of `tests/route_default_format_dispatch.rs`: the
//! server's dispatcher (`runtime/spinel/scaffold/main.rb`) applies a
//! route's default format only to a request with no extension, and
//! writes it to `params[:format]` (rubys/roundhouse#1237).

use super::{default_format_dispatch as contract, native_http};

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_default_format_fills_in_only_what_the_request_left_out_natively() {
    let (tree, errors) = contract::overlay().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    for (path, expected) in contract::CASES {
        let response = server.get(path);
        assert_eq!((response.status, response.body.as_str()), (200, expected), "{path}\n{}", server.log());
    }
}
