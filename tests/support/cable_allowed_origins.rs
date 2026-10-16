//! One Action Cable origin contract shared by the interpreted (CRuby
//! overlay) and native (spinel) lanes: the `/cable` handshake check over
//! an app's `config.action_cable` settings, environment by environment.
//!
//! The overlay assigns all three settings the way an app does: the list
//! in `config/application.rb`, `nil` in development (where Rails' engine
//! then puts its localhost default), `allow_same_origin_as_host = false`
//! in production, `disable_request_forgery_protection = true` in test.
//! `staging` has no file of its own and reads application.rb alone.
//!
//! `EXPECTED` is what actioncable 8.1.4's
//! `ActionCable::Connection::Base#allow_request_origin?` answers for the
//! same settings, Host `blog.test` and each origin in `ORIGINS` (the
//! last one absent: no Origin header). `tests/action_cable_allowed_origins.rs`
//! holds Rails itself to this table.

pub const ORIGINS: [&str; 7] = [
    "https://app.example",
    "https://chat.example.net",
    "http://chat.example.net",
    "https://chat.example.net.evil",
    "http://blog.test",
    "http://localhost:3000",
    "",
];

/// Per environment: (name, Rails' answer for each of `ORIGINS`).
pub const EXPECTED: [(&str, [bool; 7]); 4] = [
    ("production", [true, true, false, false, false, false, false]),
    ("development", [false, false, false, false, true, true, false]),
    ("test", [true, true, true, true, true, true, true]),
    ("staging", [true, true, false, false, true, false, false]),
];

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .edit(
            "config/application.rb",
            "    config.load_defaults 8.1\n",
            "    config.load_defaults 8.1\n    config.action_cable.allowed_request_origins = [\n      \"https://app.example\",\n      %r{\\Ahttps://[\\w.-]+\\.example\\.net\\z}\n    ]\n",
        )
        .edit(
            "config/environments/development.rb",
            "Rails.application.configure do\n",
            "Rails.application.configure do\n  config.action_cable.allowed_request_origins = nil\n",
        )
        .edit(
            "config/environments/production.rb",
            "Rails.application.configure do\n",
            "Rails.application.configure do\n  config.action_cable.allow_same_origin_as_host = false\n",
        )
        .edit(
            "config/environments/test.rb",
            "Rails.application.configure do\n",
            "Rails.application.configure do\n  config.action_cable.disable_request_forgery_protection = true\n",
        )
}

/// The script both lanes run: every environment, every origin, against
/// the runtime's check with the request's own origin `http://blog.test`.
pub fn script() -> String {
    let origins: Vec<String> = ORIGINS.iter().map(|o| format!("{o:?}")).collect();
    let mut out = format!("origins = [{}]\nfailures = []\n", origins.join(", "));
    for (env, expected) in EXPECTED {
        let row: Vec<String> = expected.iter().map(|b| b.to_string()).collect();
        out.push_str(&format!(
            "Rails.env_name = {env:?}\n\
             host = ActionController::RequestForgeryProtection.host_origin_for(\"blog.test\", \"\", \"\", \"\")\n\
             got = origins.map {{ |o| ActionController::RequestForgeryProtection.cable_origin_allowed?(o, host) }}\n\
             failures << \"{env}: #{{got.inspect}}\" unless got == [{}]\n",
            row.join(", ")
        ));
    }
    out.push_str("raise failures.join(\"\\n\") unless failures.empty?\nputs \"Action Cable origin contract passed\"\n");
    out
}
