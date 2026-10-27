//! Rails' encrypted session cookie on the CRuby overlay: the shared
//! contract in `support/encrypted_session_cookie.rs`, driven through the
//! overlay's own Rack entry (`Main.run_rack`) and its `/cable` identity
//! check (`Cable.identify`, whose nil `config.ru` answers with 401). The
//! native twin is `spinel_toolchain/encrypted_session_cookie.rs`.

use super::encrypted_session_cookie_contract as contract;

/// A sign-in-like action writes the session, an Action Cable connection
/// reads it through `cookies.encrypted[session_options[:key]]` and
/// accepts the handshake, the flash rides in the session and shows once,
/// and a session cookie Rails minted reads here; Rails reads the one
/// written here as the same Hash, value types and flash included.
#[test]
fn the_session_cookie_is_rails_encrypted_cookie_store() {
    let run = contract::overlay().run_ruby(&ruby_script());
    run.assert_passes();
    let (transcript, written) = run
        .stdout
        .split_once("RAW=")
        .unwrap_or_else(|| panic!("no RAW= line\nstdout:\n{}\nstderr:\n{}", run.stdout, run.stderr));
    assert_eq!(transcript, contract::EXPECTED, "stderr:\n{}", run.stderr);
    if !contract::rails_available() {
        eprintln!("skipping the Rails read: ruby with railties and actionpack not available");
        return;
    }
    let rails = contract::rails_reads(written.trim());
    assert_eq!(contract::mask_session_id(&rails), contract::RAILS_READS, "Rails read: {rails}");
}

fn ruby_script() -> String {
    format!(
        r#"require "cgi"
# cable.rb requires nio4r and websocket-driver, the reactor's gems, which
# the unit job does not install; `Cable.identify` never reaches the
# reactor, so the features are marked loaded and left empty, as in
# tests/overlay_cable_identity.rb.
%w[nio websocket/driver].each {{ |feature| $LOADED_FEATURES << feature + ".rb" }}
module NIO; end
module WebSocket; end
require File.expand_path("cable", Dir.pwd)
Rails.secret_key_base = {secret:?}
RAILS_COOKIE = {rails:?}

def get(path, cookie)
  env = {{ "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
          "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("") }}
  env["HTTP_COOKIE"] = "{key}=" + cookie unless cookie.nil?
  Main.run_rack(env)
end

# The session cookie a response set, as the browser sends it back.
def session_cookie(headers)
  Array(headers["set-cookie"]).map {{ |line| line[/\A{key}=([^;]*)/, 1] }}.compact.first
end

def cable(cookie)
  env = {{ "PATH_INFO" => "/cable" }}
  env["HTTP_COOKIE"] = "{key}=" + cookie unless cookie.nil?
  Cable.identify(env).nil? ? "refused" : "accepted"
end

status, headers, _ = get("/sign-in", nil)
signed_in = session_cookie(headers)
puts "sign-in #{{status}} #{{signed_in ? "sets {key}" : "sets no session"}}"
puts "cable with the session: #{{cable(signed_in)}}"
puts "cable without a cookie: #{{cable(nil)}}"
tampered = (signed_in[0] == "A" ? "B" : "A") + signed_in[1..]
puts "cable with a tampered cookie: #{{cable(tampered)}}"
_, headers, body = get("/whoami", signed_in)
puts "whoami: #{{body.join}}"
shown = session_cookie(headers) || signed_in
_, _, body = get("/whoami", shown)
puts "whoami again: #{{body.join}}"
rails = CGI.escape(RAILS_COOKIE)
_, _, body = get("/whoami", rails)
puts "rails cookie whoami: #{{body.join}}"
puts "rails cookie cable: #{{cable(rails)}}"
print "RAW=" + CGI.unescape(signed_in) + "\n"
"#,
        secret = contract::SECRET,
        rails = contract::RAILS_COOKIE,
        key = contract::KEY,
    )
}
