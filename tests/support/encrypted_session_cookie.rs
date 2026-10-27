//! The session cookie contract shared by the interpreted (CRuby overlay)
//! and native (spinel) lanes: the emitted app reads and writes the session
//! as Rails 8.1.4's `CookieStore` does, so an Action Cable connection that
//! authorizes from `cookies.encrypted[session_options[:key]]` accepts the
//! handshake a sign-in-like action made possible.
//!
//! Before this contract the runtime SIGNED its own `k=v` encoding of the
//! session, there was no `cookies.encrypted`, and such a connection
//! refused every handshake (401; the browser reconnects forever) while
//! the same app worked under Puma.
//!
//! Each lane drives the same steps and prints the same transcript
//! (`EXPECTED`); `tests/support/rails_session_oracle.rb` is Rails itself,
//! reading what the emitted app wrote and minting `RAILS_COOKIE`.

#![allow(dead_code)]

/// The app's `secret_key_base`, the oracle's too.
pub const SECRET: &str = "roundhouse-encrypted-session-cookie-contract-secret";

/// The session cookie's name, as the app declares it.
pub const KEY: &str = "_blog_session";

/// A session cookie Rails 8.1.4 wrote (`rails_session_oracle.rb mint`):
/// `authenticated_at` 1760000000, `user` "grace", and the flash message
/// "From Rails", under `SECRET`.
pub const RAILS_COOKIE: &str = "n/E3itO9wpc9wRej+pu+tq9B9ca8eJAdmYHrV59HMtWxKgfiRN34RXmIIMBx7Eq3vXOl4a/isNbyLE/rq9czpoVy+twTgSIcpKe8/upuH3mYrpKTpOQ++wA53qxCqhG4w3ZgGszrkZCbxklFLUi5a1/ZN1qN9emxHVI5EKZq/xQNsGlvwKTegKqs4GidGR1yveJPo6q6t8D1NVr02IqXc8xEGtx0E/snERWCOWk6KARxIJLwwQKBkBitpKjiBJqmiaaYT2QbvEgooFwibIHzRxgBiKb7gzFteslwX2o1KV3UuOHIjdOYElViDs9CD5lYP+ttWD3Lg+t9SKHxC58+4c66lyyJSMhJxupXK/c=--nmjnRLhaD9eNk0V6--l1b+YtqaC6zcOpTfwHue4A==";

/// The app: a sign-in-like action that writes the session (an Integer
/// and a String) and a flash message, a page that shows them, and the
/// connection an app whose cable reads the session writes.
pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "config/initializers/session_store.rb",
            "Rails.application.config.session_store :cookie_store, key: \"_blog_session\"\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/sign-in\", to: \"sign_ins#create\"\n  get \"/whoami\", to: \"sign_ins#show\"\n",
        )
        .write(
            "app/controllers/sign_ins_controller.rb",
            r#"class SignInsController < ApplicationController
  def create
    session[:authenticated_at] = 1760000000
    session[:user] = "ada"
    redirect_to "/whoami", notice: "Signed in"
  end

  def show
    render plain: "user=#{session[:user]} at=#{session[:authenticated_at]} notice=#{flash[:notice]}"
  end
end
"#,
        )
        .write(
            "app/channels/application_cable/connection.rb",
            r#"module ApplicationCable
  class Connection < ActionCable::Connection::Base
    identified_by :current_user

    def connect
      self.current_user = find_verified_user
    end

    private

    def find_verified_user
      session = cookies.encrypted[Rails.application.config.session_options[:key]]
      if session && session["authenticated_at"].is_a?(Integer) && session["authenticated_at"] > 1_700_000_000
        session["user"]
      else
        reject_unauthorized_connection
      end
    end
  end
end
"#,
        )
}

/// What each lane prints, one line per step:
///
/// 1. `GET /sign-in` with no cookie: a redirect that sets the session.
/// 2. The `/cable` handshake with that cookie, without one, and with the
///    cookie's last character changed.
/// 3. `GET /whoami` with it: the session's values and the flash, shown
///    once; then again with the cookie that response set.
/// 4. The cookie Rails minted: the same page, and the handshake.
pub const EXPECTED: &str = "sign-in 302 sets _blog_session
cable with the session: accepted
cable without a cookie: refused
cable with a tampered cookie: refused
whoami: user=ada at=1760000000 notice=Signed in
whoami again: user=ada at=1760000000 notice=
rails cookie whoami: user=grace at=1760000000 notice=From Rails
rails cookie cable: accepted
";

/// What Rails reads from the cookie step 1 set (`rails_session_oracle.rb
/// read`), with the 32-hex `session_id` the store added written as
/// `SID`: the session, `cookies.encrypted[key]` (the same Hash) and the
/// flash message.
pub const RAILS_READS: &str = r#"{"session":{"session_id":"SID","authenticated_at":1760000000,"user":"ada","flash":{"discard":[],"flashes":{"notice":"Signed in"}}},"encrypted":{"session_id":"SID","authenticated_at":1760000000,"user":"ada","flash":{"discard":[],"flashes":{"notice":"Signed in"}}},"notice":"Signed in"}"#;

/// `json` with each `"session_id":"<32 hex>"` written as `SID`, so two
/// readings of one cookie compare equal whatever id the store drew; a
/// value of another shape is left as it is and fails the comparison.
pub fn mask_session_id(json: &str) -> String {
    let marker = "\"session_id\":\"";
    let mut out = String::new();
    let mut rest = json;
    while let Some(at) = rest.find(marker) {
        let start = at + marker.len();
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let id: String = tail.chars().take_while(|c| *c != '"').collect();
        if id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
            out.push_str("SID");
        } else {
            out.push_str(&id);
        }
        rest = &tail[id.len()..];
    }
    out.push_str(rest);
    out
}

/// Whether this machine's ruby has the gems the Rails oracle loads
/// (railties and actionpack 8.1). Where it has not, each lane checks
/// its transcript and skips the Rails read, as the other oracle tests
/// do (`tests/action_cable_allowed_origins.rs`).
pub fn rails_available() -> bool {
    std::process::Command::new("ruby")
        .args(["-rrails", "-raction_controller/railtie", "-e", "1"])
        .status()
        .is_ok_and(|s| s.success())
}

/// Rails' reading of `raw` (unescaped), through the oracle.
pub fn rails_reads(raw: &str) -> String {
    let oracle = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_session_oracle.rb");
    let output = std::process::Command::new("ruby")
        .arg(&oracle)
        .arg("read")
        .arg(raw)
        .output()
        .expect("ruby is on PATH");
    assert!(
        output.status.success(),
        "the Rails oracle failed\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// `%XX` escapes decoded, as Rack decodes a cookie value.
pub fn unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8(out).expect("a cookie value is UTF-8")
}

/// `raw` as Rack escapes a cookie value (`Rack::Utils.escape`): every
/// byte but the unreserved ones as `%XX`, which is how a browser holds
/// the value Rails set.
pub fn escape(raw: &str) -> String {
    raw.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
