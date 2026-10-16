//! A JSON body's params on the CRuby overlay lane, against Rails 8.1.4's
//! answers (see `tests/support/json_params.rs`).

use super::json_params_contract as contract;

#[test]
fn a_json_body_reaches_params_as_rails_parses_it() {
    let mut script = String::from(
        r##"
require "stringio"
ActionController::Base.allow_forgery_protection = false
def post_json(body)
  env = {
    "REQUEST_METHOD" => "POST", "PATH_INFO" => "/echo", "QUERY_STRING" => "",
    "CONTENT_TYPE" => "application/json",
    "CONTENT_LENGTH" => body.bytesize.to_s, "HTTP_ACCEPT" => "text/plain"
  }
  status, body = Main.dispatch_core(env, StringIO.new(body))
  [status, body]
end
"##,
    );
    for (body, expected) in contract::CASES {
        script.push_str(&format!(
            "status, text = post_json({body:?})\nwant = {expected:?}\nraise \"expected 200 and #{{want.inspect}}, got #{{status}} #{{text.inspect}}\" unless status == 200 && text == want\n"
        ));
    }
    script.push_str(&format!(
        "status, _ = post_json({:?})\nraise \"malformed JSON: expected 400 as Rails answers, got #{{status}}\" unless status == 400\nputs \"json params as Rails\"\n",
        contract::MALFORMED
    ));
    contract::blog().run_ruby(&script).assert_passes();
}
