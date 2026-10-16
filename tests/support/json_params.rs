//! A JSON request body's params, as Rails gives them: nested objects and
//! arrays kept, JSON scalars with their types (`3`, `true`, `null`), a
//! body that is not an object under `_json`, Rails' deep_munge dropping
//! the nils inside an array, and a body that is not JSON answered 400.
//! One action echoes what it reads; the expected lines are what
//! actionpack 8.1.4 (`ActionDispatch::Request#parameters` wrapped in
//! `ActionController::Parameters`) makes of the same bodies.
//!
//! The first body is the shape of a WebAuthn passkey registration:
//! `Array(params.dig(:response, :transports)).map(&:to_s).to_json` and a
//! null `userHandle`.

pub const ROUTE_FIND: &str = "  root \"articles#index\"\n";
pub const ROUTE_REPLACE: &str = "  root \"articles#index\"\n  post \"echo\" => \"articles#echo\"\n";

pub const ACTION_FIND: &str = "  private\n";
pub const ACTION_REPLACE: &str = r##"  def echo
    transports = Array(params.dig(:response, :transports)).map(&:to_s).to_json
    handle = params.dig(:response, :userHandle).nil? ? "nil" : "present"
    render plain: "#{transports} #{handle} #{params.dig(:response, :n)} #{params.dig(:response, :ok)} #{params[:nils].to_json} #{params[:list].to_json} #{params[:_json].to_json}"
  end

  private
"##;

/// (body, expected plain-text answer) pairs; Rails 8.1.4's answers.
pub const CASES: &[(&str, &str)] = &[
    (
        r#"{"response":{"transports":["internal","hybrid"],"userHandle":null,"n":3,"ok":true},"nils":[null],"list":[1,null,"x"]}"#,
        r#"["internal","hybrid"] nil 3 true [] [1,"x"] null"#,
    ),
    (r#"["a",null,"b"]"#, r#"[] nil   null null ["a","b"]"#),
];

/// A body that is not JSON: Rails raises ParseError and answers 400.
pub const MALFORMED: &str = r#"{"response":{"n":1"#;

/// real-blog with the echo route and action.
pub fn blog() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .edit("config/routes.rb", ROUTE_FIND, ROUTE_REPLACE)
        .edit("app/controllers/articles_controller.rb", ACTION_FIND, ACTION_REPLACE)
}
