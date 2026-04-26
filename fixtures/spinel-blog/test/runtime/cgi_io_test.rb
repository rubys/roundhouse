require_relative "../test_helper"
require "cgi_io"
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
end
