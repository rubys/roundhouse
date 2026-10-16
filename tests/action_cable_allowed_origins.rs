//! Rails itself against the Action Cable origin table the emitted lanes
//! are held to (`tests/support/cable_allowed_origins.rs`): actioncable
//! 8.1.4's `ActionCable::Connection::Base#allow_request_origin?`, run on a
//! mock `/cable` request for each environment's effective settings. The
//! settings below are what the contract's overlay assigns, after Rails'
//! load order (application.rb, the environment file, the engine's
//! development default, initializers).

#[allow(dead_code)]
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[allow(dead_code)]
#[path = "support/cable_allowed_origins.rs"]
mod cable_allowed_origins;

use std::process::Command;

#[test]
fn rails_answers_the_contract_table() {
    if !Command::new("ruby").args(["-raction_cable", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the actioncable gem not available");
        return;
    }
    let origins: Vec<String> = cable_allowed_origins::ORIGINS
        .iter()
        .map(|o| if o.is_empty() { "nil".to_string() } else { format!("{o:?}") })
        .collect();
    let mut script = String::from(
        r#"require "action_cable"
require "logger"
require "rack/mock"

def rails_allows?(origin, allowed, same, disable)
  config = ActionCable::Server::Configuration.new
  config.allowed_request_origins = allowed
  config.allow_same_origin_as_host = same
  config.disable_request_forgery_protection = disable
  config.logger = Logger.new(nil)
  server = ActionCable::Server::Base.new(config: config)
  headers = { "HTTP_HOST" => "blog.test" }
  headers["HTTP_ORIGIN"] = origin unless origin.nil?
  env = Rack::MockRequest.env_for("/cable", headers)
  ActionCable::Connection::Base.new(server, env).send(:allow_request_origin?)
end

listed = ["https://app.example", %r{\Ahttps://[\w.-]+\.example\.net\z}]
settings = {
  "production" => [listed, false, false],
  "development" => [/https?:\/\/localhost:\d+/, true, false],
  "test" => [listed, true, true],
  "staging" => [listed, true, false],
}
"#,
    );
    script.push_str(&format!("origins = [{}]\n", origins.join(", ")));
    for (env, expected) in cable_allowed_origins::EXPECTED {
        let row: Vec<String> = expected.iter().map(|b| b.to_string()).collect();
        script.push_str(&format!(
            "got = origins.map {{ |o| rails_allows?(o, *settings.fetch({env:?})) }}\n\
             raise \"{env}: Rails answers #{{got.inspect}}\" unless got == [{}]\n",
            row.join(", ")
        ));
    }
    script.push_str("puts \"Rails agrees\"\n");
    let output = Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Rails agrees"),
        "actioncable 8.1.4 disagrees with the contract table\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}
