//! #1699: Python's dispatch glue stripped a registered extension itself
//! before `Router.match`, so the router's extension pass, and the format
//! requirement it checks, never ran: `/exports/1/export.json` on a route
//! held to `format: /csv/` was served (Rails 404s it). `handle` now hands
//! the router the path as requested and takes the format it reports, the
//! extension ahead of a route's default as in Rails (`/feeds/1.xml` on a
//! `defaults: { format: :json }` route is `xml`).

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn dispatch_handle() -> String {
    let (out, _) = emit_and_run::real_blog().emit(BuildTarget::Python);
    let source = std::fs::read_to_string(out.join("app/v2/dispatch.py")).unwrap();
    let start = source.find("def handle(").expect("handle");
    let rest = &source[start..];
    let end = rest[1..].find("\ndef ").map(|i| i + 1).unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn python_dispatch_hands_the_router_the_requested_path() {
    let handle = dispatch_handle();
    assert!(!handle.contains("format_extension"), "the glue strips an extension itself:\n{handle}");
    assert!(handle.contains("m = Router.match(method, path, _route_table())"), "{handle}");
    // The path's extension, then the route's default, then a `format` in
    // the request's own params (`cr_oracle.out`: a POST body's
    // `format=html` loses to a route defaulting to JSON).
    assert!(handle.contains("fmt = m.path_params.get(\"format\") or m.req_format or p.get(\"format\") or \"html\""), "{handle}");
}
