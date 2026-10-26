//! A route's default format fills in only what the request left out
//! (rubys/roundhouse#1237). Before, `defaults: { format: :json }` forced
//! the response format over the path's extension (`/widgets/1.html` was
//! answered from the json arm) and never reached `params[:format]`. The
//! expectations are Rails 8.1.4's (table in `support/default_format_dispatch.rs`).
//! CI installs neither gem, so the oracle is not rerun here.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/default_format_dispatch.rs"]
mod default_format_dispatch;

#[test]
fn a_default_format_fills_in_only_what_the_request_left_out() {
    let expected: Vec<String> = default_format_dispatch::CASES
        .iter()
        .map(|(path, body)| format!("{path:?} => {body:?}"))
        .collect();
    default_format_dispatch::overlay()
        .run_ruby(&format!(
            r##"
require "stringio"
expected = {{ {} }}
actual = {{}}
expected.each_key do |path|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each {{ |part| text << part }}
  actual[path] = status == 200 ? text : "#{{status}} #{{text}}"
end
raise "expected #{{expected.inspect}}, got #{{actual.inspect}}" unless actual == expected
puts "ALL OK"
"##,
            expected.join(", ")
        ))
        .assert_passes();
}
