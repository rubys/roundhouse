//! Rails' encrypted session cookie on Spinel: the shared contract in
//! `support/encrypted_session_cookie.rs`, over the emitted binary's HTTP
//! server, the `/cable` handshake included (101 or 401). The overlay twin
//! is `emit_and_run/encrypted_session_cookie.rs`.

use super::encrypted_session_cookie_contract as contract;
use super::native_http;
use roundhouse::project::BuildTarget;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn the_session_cookie_is_rails_encrypted_cookie_store_natively() {
    let (tree, errors) = contract::overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors:\n{}", errors.join("\n"));
    native_http::build(&tree);
    let server = native_http::Server::start_with_env(&tree, &[("SECRET_KEY_BASE", contract::SECRET)]);

    let cookie_header = |value: &str| format!("{}={value}", contract::KEY);
    let session_cookie = |set_cookies: &[String]| {
        set_cookies.iter().find_map(|line| {
            line.split(';').next().and_then(|pair| pair.strip_prefix(&format!("{}=", contract::KEY))).map(String::from)
        })
    };
    let get = |path: &str, cookie: Option<&str>| {
        let header = cookie.map(cookie_header).unwrap_or_default();
        server.exchange("GET", path, &[("Cookie", header.as_str())])
    };
    let cable = |cookie: Option<&str>| {
        let header = cookie.map(cookie_header).unwrap_or_default();
        match server.handshake("/cable", &[("Origin", "http://127.0.0.1"), ("Cookie", header.as_str())]) {
            101 => "accepted".to_string(),
            401 => "refused".to_string(),
            other => format!("answered {other}"),
        }
    };

    let mut transcript = String::new();
    let (status, set_cookies, _) = get("/sign-in", None);
    let signed_in = session_cookie(&set_cookies);
    transcript.push_str(&format!(
        "sign-in {status} {}\n",
        if signed_in.is_some() { format!("sets {}", contract::KEY) } else { "sets no session".into() }
    ));
    let signed_in = signed_in.unwrap_or_else(|| panic!("no session cookie in {set_cookies:?}\n{}", server.log()));
    transcript.push_str(&format!("cable with the session: {}\n", cable(Some(&signed_in))));
    transcript.push_str(&format!("cable without a cookie: {}\n", cable(None)));
    let tampered = format!("{}{}", if signed_in.starts_with('A') { "B" } else { "A" }, &signed_in[1..]);
    transcript.push_str(&format!("cable with a tampered cookie: {}\n", cable(Some(&tampered))));
    let (_, set_cookies, body) = get("/whoami", Some(&signed_in));
    transcript.push_str(&format!("whoami: {body}\n"));
    let shown = session_cookie(&set_cookies).unwrap_or_else(|| signed_in.clone());
    let (_, _, body) = get("/whoami", Some(&shown));
    transcript.push_str(&format!("whoami again: {body}\n"));
    let rails = contract::escape(contract::RAILS_COOKIE);
    let (_, _, body) = get("/whoami", Some(&rails));
    transcript.push_str(&format!("rails cookie whoami: {body}\n"));
    transcript.push_str(&format!("rails cookie cable: {}\n", cable(Some(&rails))));

    assert_eq!(transcript, contract::EXPECTED, "server log:\n{}", server.log());
    if !contract::rails_available() {
        eprintln!("skipping the Rails read: ruby with railties and actionpack not available");
        return;
    }
    let read = contract::rails_reads(&contract::unescape(&signed_in));
    assert_eq!(contract::mask_session_id(&read), contract::RAILS_READS, "Rails read: {read}");
}
