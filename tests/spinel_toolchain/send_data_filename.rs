//! `send_data(filename:)` over the native HTTP server; the Ruby-lane
//! twin is `emit_and_run/send_data_filename.rs`.

use super::native_http;
use super::send_data_filename_contract as contract;
use roundhouse::project::BuildTarget;

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn send_data_filename_is_observable_in_the_native_http_response() {
    let (tree, errors) = contract::overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors:\n{}", errors.join("\n"));
    native_http::build(&tree);
    let server = native_http::Server::start(&tree);

    let expected = [
        ("/downloads/tarball", "attachment; filename=\"backup.tar.gz\"; filename*=UTF-8''backup.tar.gz"),
        ("/downloads/report", "inline; filename=\"my file %281%29.txt\"; filename*=UTF-8''my%20file%20%281%29.txt"),
        ("/downloads/utf8", "attachment; filename=\"za%3F%3F%3F%3F.pdf\"; filename*=UTF-8''za%C5%BC%C3%B3%C5%82%C4%87.pdf"),
        ("/downloads/bare", "inline"),
    ];
    for (route, header) in expected {
        let response = server.get(route);
        assert_eq!(response.status, 200, "{route}: {}", server.log());
        assert_eq!(
            response.headers.get("content-disposition").map(String::as_str),
            Some(header),
            "{route}: {:?}",
            response.headers
        );
    }
}
