//! Shared emitted response fixture for Ruby and native Spinel.
use super::emit_and_run;

const CONTROLLER: &str = include_str!("response_content_type/controller.rb");
pub const PROGRAM: &str = include_str!("response_content_type/program.rb");
pub const EXPECTED: &str = include_str!("response_content_type/expected.json");
const ROUTES: &str = r#"Rails.application.routes.draw do
  get "/json", to: "content_probes#json"
  get "/plain", to: "content_probes#plain"
  get "/html", to: "content_probes#html"
  get "/vendor_json", to: "content_probes#vendor_json"
  get "/json_charset", to: "content_probes#json_charset"
  get "/head", to: "content_probes#head_ok"
  get "/head_csv", to: "content_probes#head_csv"
  get "/send_csv", to: "content_probes#send_csv"
  get "/send_binary", to: "content_probes#send_binary"
  get "/redirect", to: "content_probes#redirect"
  get "/plain_csv", to: "content_probes#plain_csv"
  get "/plain_charset", to: "content_probes#plain_charset"
  get "/head_json", to: "content_probes#head_json"
  get "/head_no_content", to: "content_probes#head_no_content"
  get "/send_json", to: "content_probes#send_json"
  get "/inherited_charset", to: "content_probes#inherited_charset"
  get "/new_symbol_type", to: "content_probes#new_symbol_type"
  get "/inherited_trailing_parameter", to: "content_probes#inherited_trailing_parameter"
  get "/inherited_quoted", to: "content_probes#inherited_quoted"
  get "/inherited_trailing_space", to: "content_probes#inherited_trailing_space"
  get "/inherited_uppercase_value", to: "content_probes#inherited_uppercase_value"
  get "/inherited_uppercase_key", to: "content_probes#inherited_uppercase_key"
  get "/inherited_no_charset", to: "content_probes#inherited_no_charset"
  get "/inherited_empty_charset", to: "content_probes#inherited_empty_charset"
  get "/inherited_other_parameter", to: "content_probes#inherited_other_parameter"
  get "/inherited_duplicate_charset", to: "content_probes#inherited_duplicate_charset"
  get "/inherited_boundary_before", to: "content_probes#inherited_boundary_before"
  get "/new_quoted_charset", to: "content_probes#new_quoted_charset"
  get "/new_trailing_parameter", to: "content_probes#new_trailing_parameter"
  get "/new_uppercase_value", to: "content_probes#new_uppercase_value"
  get "/new_uppercase_key", to: "content_probes#new_uppercase_key"
  get "/new_empty_charset", to: "content_probes#new_empty_charset"
end
"#;

pub fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/content_probes_controller.rb", CONTROLLER)
        .write("config/routes.rb", ROUTES)
}
