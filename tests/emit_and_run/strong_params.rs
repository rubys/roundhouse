//! Strong parameters refuse a request the way Rails does: a missing,
//! malformed or blank resource answers 400 (`ActionController::ParameterMissing`),
//! and `expect` accepts each nested key only with the kind of value its
//! filter takes. The Rails answers are measured on actionpack 8.1.4.

use super::emit_and_run;

/// `params.expect(article: …)` with the resource key missing or not a
/// Hash raises `ActionController::ParameterMissing`, which Rails answers
/// with 400 when the app does not rescue it. Form-encoded posts, which
/// Rails never wraps, to the scaffold's `create`; measured on the
/// fixture under Rails 8.1.4: 400, 400, and 201 for the nested form.
#[test]
fn a_missing_resource_key_answers_400() {
    emit_and_run::real_blog()
        .run_ruby(
            r#"
require "stringio"
# CSRF off, as Rails' test environment has it (the Rails measurements
# above were taken that way); this is about the action, not the token.
ActionController::Base.allow_forgery_protection = false
def post_form(path, body)
  env = {
    "REQUEST_METHOD" => "POST", "PATH_INFO" => path, "QUERY_STRING" => "",
    "CONTENT_TYPE" => "application/x-www-form-urlencoded",
    "CONTENT_LENGTH" => body.bytesize.to_s, "HTTP_ACCEPT" => "application/json"
  }
  status, _body = Main.dispatch_core(env, StringIO.new(body))
  status
end
status = post_form("/articles.json", "title=Flat&body=A+body+long+enough")
raise "no article key: expected 400 as Rails answers, got #{status.inspect}" unless status == 400
status = post_form("/articles.json", "article=oops")
raise "article is a scalar: expected 400 as Rails answers, got #{status.inspect}" unless status == 400
status = post_form("/articles.json", "article%5Btitle%5D=Nested&article%5Bbody%5D=A+body+long+enough")
raise "article nested: expected 201 as Rails answers, got #{status.inspect}" unless status == 201
raise "the nested article was not created" unless Article.find_by(title: "Nested")
puts "missing resource key answers 400"
"#,
        )
        .assert_passes();
}

/// `params.require(:article).permit(…)` refuses less than `expect`, as
/// in Rails: a missing key raises `ParameterMissing` (400), a hash of
/// only unpermitted keys passes and fails validation (422), and a scalar
/// reaches `permit`, which a String does not have (500). Measured on the
/// fixture with its `article_params` switched to this form, Rails 8.1.4:
/// 400, 500, 422, 201.
#[test]
fn require_permit_refuses_like_rails() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "params.expect(article: [ :title, :body ])",
            "params.require(:article).permit(:title, :body)",
        )
        .run_ruby(
            r##"
require "stringio"
# CSRF off, as Rails' test environment has it (the Rails measurements
# above were taken that way); this is about the action, not the token.
ActionController::Base.allow_forgery_protection = false
def post(path, type, body)
  env = {
    "REQUEST_METHOD" => "POST", "PATH_INFO" => path, "QUERY_STRING" => "",
    "CONTENT_TYPE" => type,
    "CONTENT_LENGTH" => body.bytesize.to_s, "HTTP_ACCEPT" => "application/json"
  }
  status, _body = Main.dispatch_core(env, StringIO.new(body))
  status
rescue NoMethodError
  500
end
FORM = "application/x-www-form-urlencoded"
JSON_TYPE = "application/json"
# Rails refuses a blank resource (`blank?`: whitespace, an empty array)
# with ParameterMissing; anything else that is not a hash reaches
# `permit` and is a NoMethodError.
expected = [
  ["no article key", FORM, "title=Flat&body=A+body+long+enough", 400],
  ["article is whitespace", FORM, "article=%20%20", 400],
  ["article is a tab", FORM, "article=%09", 400],
  ["article is an empty array", JSON_TYPE, %({"article":[]}), 400],
  ["article is a scalar", FORM, "article=oops", 500],
  ["article is an array of a blank", JSON_TYPE, %({"article":[""]}), 500],
  ["article is false", JSON_TYPE, %({"article":false}), 500],
  ["only unpermitted keys", FORM, "article%5Bother%5D=1", 422],
  ["article nested", FORM, "article%5Btitle%5D=Nested&article%5Bbody%5D=A+body+long+enough", 201],
]
expected.each do |label, type, body, want|
  status = post("/articles.json", type, body)
  raise "#{label}: expected #{want} as Rails answers, got #{status.inspect}" unless status == want
end
puts "require.permit refuses like Rails"
"##,
        )
        .assert_passes();
}

/// `Params.expect_present` accepts a nested key only with the kind of
/// value its filter takes. Every row is Rails 8.1.4's
/// `ActionController::Parameters#expect` on `{"article" => {key => value}}`.
#[test]
fn expect_nested_filters_accept_what_rails_accepts() {
    emit_and_run::real_blog()
        .run_ruby(
            r##"
# filter kind => [scalars, scalar arrays, hashes, arrays] for key "k"
KINDS = {
  "settings: [:theme]" => [[], [], ["k"], []],
  "tags: []" => [[], ["k"], [], []],
  "items: [[:name]]" => [[], [], [], ["k"]],
}
VALUES = {
  "hash with key" => {"theme" => "dark"}, "hash other key" => {"x" => "1"}, "empty hash" => {},
  "array of str" => ["a"], "array of hash" => [{"name" => "n"}], "empty array" => [], "scalar" => "s",
}
RAILS_ACCEPTS = {
  "settings: [:theme]" => ["hash with key", "hash other key", "empty hash"],
  "tags: []" => ["array of str", "empty array"],
  "items: [[:name]]" => ["array of str", "array of hash", "empty array"],
}
KINDS.each do |kind, (scalars, scalar_arrays, hashes, arrays)|
  VALUES.each do |name, value|
    accepted = begin
      Params.expect_present({"article" => {"k" => value}}, "article", scalars, scalar_arrays, hashes, arrays)
      true
    rescue ActionController::ParameterMissing
      false
    end
    want = RAILS_ACCEPTS[kind].include?(name)
    raise "#{kind} with #{name}: Rails #{want ? "accepts" : "refuses"}, got #{accepted ? "accepted" : "refused"}" unless accepted == want
  end
end
puts "nested expect filters match Rails"
"##,
        )
        .assert_passes();
}
