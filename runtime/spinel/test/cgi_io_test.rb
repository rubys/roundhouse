# Minitest-shaped: this is a CRuby-only framework test, quarantined
# out of the spin shape by `project.rs::spin_shape` (it is not a spin
# test program and gets no snapshot). It therefore loads its own
# runner — `test_helper` deliberately does not, because the EMITTED
# tests inherit `TestBase` and carry their own driver shim.
require "minitest/autorun"
require_relative "test_helper"
require_relative "../runtime/cgi_io"
require "stringio"

class CgiIoTest < Minitest::Test
  # ── parse_request: cookies ───────────────────────────────────────

  def test_parse_request_no_cookies_returns_empty_hash
    env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => "/" }
    req = CgiIo.parse_request(env, StringIO.new)
    assert_equal({}, req[:cookies])
  end

  def test_parse_request_single_cookie
    env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => "/", "HTTP_COOKIE" => "foo=bar" }
    req = CgiIo.parse_request(env, StringIO.new)
    assert_equal "bar", req[:cookies][:foo]
  end

  def test_parse_request_multiple_cookies
    env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => "/", "HTTP_COOKIE" => "a=1; b=2; c=3" }
    req = CgiIo.parse_request(env, StringIO.new)
    assert_equal "1", req[:cookies][:a]
    assert_equal "2", req[:cookies][:b]
    assert_equal "3", req[:cookies][:c]
  end

  def test_parse_request_url_decodes_cookie_values
    env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => "/", "HTTP_COOKIE" => "msg=Hello%20World" }
    req = CgiIo.parse_request(env, StringIO.new)
    assert_equal "Hello World", req[:cookies][:msg]
  end

  def test_parse_request_handles_extra_whitespace
    env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => "/", "HTTP_COOKIE" => " a = 1 ;  b = 2 " }
    req = CgiIo.parse_request(env, StringIO.new)
    assert_equal "1", req[:cookies][:a]
    assert_equal "2", req[:cookies][:b]
  end

  # ── write_response: set_cookies ──────────────────────────────────

  # ── write_response: header injection ─────────────────────────────

  # Every header the app hands over can carry request data — the
  # Location of `redirect_to params[:back]`, a Content-Disposition, the
  # blob's own Content-Type — and a CR or LF in it would end the header
  # and write the rest as one the app never set. Puma's rule, as the
  # spinel servers apply it (Tep.header_lines): a header that cannot be
  # one line is dropped, and the response still goes out.
  def test_write_response_drops_a_header_carrying_a_control_character
    io = StringIO.new
    CgiIo.write_response(io, 302, "<p>",
      location: "/next\r\nSet-Cookie: pwned=1",
      content_type: "text/html\r\nX-Type-Injected: 1",
      extra_headers: { "Content-Disposition" => "inline\nX-Injected: 1",
                       "X-Bad\r\nX-Key-Injected" => "v",
                       "X-Ok" => "fine\tstill fine" })
    head = io.string.split("\r\n\r\n", 2).first
    lines = head.split("\r\n")
    assert lines.none? { |l| l.include?("\n") }, head.inspect
    assert lines.none? { |l| l.start_with?("Set-Cookie: pwned", "X-Injected", "X-Key-Injected", "X-Type-Injected") }, head.inspect
    assert lines.none? { |l| l.start_with?("Location:", "Content-Disposition:", "Content-Type:") }, head.inspect
    assert_includes lines, "X-Ok: fine\tstill fine"
    assert_includes lines, "Status: 302 Found"
  end

  def test_write_response_no_cookies_emits_no_set_cookie_header
    io = StringIO.new
    CgiIo.write_response(io, 200, "<p>")
    refute_includes io.string, "Set-Cookie"
  end

  def test_write_response_emits_set_cookie_with_value
    io = StringIO.new
    CgiIo.write_response(io, 200, "<p>", set_cookies: { foo: "bar" })
    assert_includes io.string, "Set-Cookie: foo=bar"
    assert_includes io.string, "Path=/"
    assert_includes io.string, "HttpOnly"
  end

  def test_write_response_url_encodes_set_cookie_value
    io = StringIO.new
    CgiIo.write_response(io, 200, "<p>", set_cookies: { msg: "Hello World!" })
    assert_includes io.string, "Set-Cookie: msg=Hello%20World%21"
  end

  def test_write_response_nil_value_clears_cookie
    io = StringIO.new
    CgiIo.write_response(io, 200, "<p>", set_cookies: { foo: nil })
    assert_includes io.string, "Set-Cookie: foo="
    assert_includes io.string, "Max-Age=0"
  end

  def test_write_response_emits_one_set_cookie_per_entry
    io = StringIO.new
    CgiIo.write_response(io, 200, "<p>", set_cookies: { a: "1", b: "2" })
    cookie_lines = io.string.scan(/^Set-Cookie:.*$/).length
    assert_equal 2, cookie_lines
  end

  # ── url_encode/decode round-trip ─────────────────────────────────

  def test_url_encode_alphanumeric_passthrough
    assert_equal "Hello123", CgiIo.url_encode("Hello123")
  end

  def test_url_encode_unreserved_chars_passthrough
    assert_equal "a-b.c_d~e", CgiIo.url_encode("a-b.c_d~e")
  end

  def test_url_encode_spaces_become_percent_20
    assert_equal "Hello%20World", CgiIo.url_encode("Hello World")
  end

  def test_url_encode_special_chars
    assert_equal "%26%3D%3B", CgiIo.url_encode("&=;")
  end

  def test_url_decode_inverse_of_encode
    samples = ["Hello", "Hello World", "a&b=c;d", "café", "Article was successfully created."]
    samples.each do |s|
      assert_equal s, CgiIo.url_decode(CgiIo.url_encode(s)), "round-trip: #{s.inspect}"
    end
  end

  # The bracket grammar `Hash#to_query` writes: one level, an array,
  # two levels, and a malformed key that is dropped rather than raised.
  def test_parse_form_into_reads_nested_and_array_keys
    into = {}
    CgiIo.parse_form_into("room%5Bname%5D=Designers&user_ids%5B%5D=1&user_ids%5B%5D=2&id=7&a%5Bb%5D%5Bc%5D=x&bad%5B=1", into)
    assert_equal({ "room" => { "name" => "Designers" }, "user_ids" => ["1", "2"], "id" => "7", "a" => { "b" => { "c" => "x" } } }, into)
  end

  # A JSON body — `@rails/request.js` with `contentType:
  # "application/json"`, campfire's link unfurl — is params too, nested
  # and typed as Rails parses it. Malformed JSON leaves the params alone.
  def test_parse_request_reads_a_json_body
    body = '{"url":"https://a.example/","blob":{"filename":"a.png","byte_size":12}}'
    env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/unfurl_link", "QUERY_STRING" => "q=1",
            "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s }
    req = CgiIo.parse_request(env, StringIO.new(body))
    assert_equal "https://a.example/", req[:params]["url"]
    assert_equal({ "filename" => "a.png", "byte_size" => 12 }, req[:params]["blob"])
    assert_equal "1", req[:params]["q"]
  end

  def test_parse_request_ignores_a_malformed_json_body
    body = "{not json"
    env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/unfurl_link",
            "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s }
    req = CgiIo.parse_request(env, StringIO.new(body))
    assert_equal({}, req[:params])
  end

  def test_parse_request_preserves_plain_text_body_for_request_body_readers
    body = "Hello from a bot"
    env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/messages",
            "CONTENT_TYPE" => "text/plain; charset=utf-8", "CONTENT_LENGTH" => body.bytesize.to_s }
    req = CgiIo.parse_request(env, StringIO.new(body))
    assert_equal body, req[:raw_body]
    assert_equal({}, req[:params])
  end

  def test_parse_request_preserves_raw_body_while_parsing_form_params
    body = "message%5Bbody%5D=hello"
    env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/messages",
            "CONTENT_TYPE" => "application/x-www-form-urlencoded", "CONTENT_LENGTH" => body.bytesize.to_s }
    req = CgiIo.parse_request(env, StringIO.new(body))
    assert_equal body, req[:raw_body]
    assert_equal "hello", req[:params]["message"]["body"]
  end
end
