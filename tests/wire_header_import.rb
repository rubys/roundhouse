require_relative "../runtime/spinel/tep/tep_core"
require_relative "../runtime/spinel/tep/url"
require_relative "../runtime/spinel/tep/request"
require_relative "../runtime/spinel/tep/parser"
require_relative "../runtime/ruby/action_dispatch/request"

if ARGV.include?("--puma")
  gem "puma", "8.0.2"
  require "puma"
end

scaffold = File.expand_path("../runtime/spinel/scaffold/main.rb", __dir__)
lines = File.readlines(scaffold)
start = lines.index { |line| line.strip == "req.req_headers.each do |name, value|" }
raise "native import loop not found" unless start && lines[start + 2].strip == "end"
copy_loop = lines[start, 3].join

cases = [
  ["rack.session", "HTTP_RACK.SESSION"],
  ["rack.url_scheme", "HTTP_RACK.URL,SCHEME"],
  ["action_dispatch.foo", "HTTP_ACTION,DISPATCH.FOO"],
  ["roundhouse.listener", "HTTP_ROUNDHOUSE.LISTENER"],
  ["puma.socket", "HTTP_PUMA.SOCKET"],
  ["HTTP_ACCEPT", "HTTP_HTTP,ACCEPT"],
  ["HTTP_X_CSRF_TOKEN", "HTTP_HTTP,X,CSRF,TOKEN"],
  ["Content_Type", "HTTP_CONTENT,TYPE"],
  ["Content_Length", "HTTP_CONTENT,LENGTH"],
  ["X-Case", "HTTP_X_CASE"],
  ["x-cAsE", "HTTP_X_CASE"],
  ["X_Case", "HTTP_X,CASE"],
  ["X0123456789abcdefghijklmnopqrstuvwxyz", "HTTP_X0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"],
]
%w[AUTH_TYPE CONTENT_LENGTH CONTENT_TYPE GATEWAY_INTERFACE HTTPS PATH_INFO
   PATH_TRANSLATED QUERY_STRING REMOTE_ADDR REMOTE_HOST REMOTE_IDENT REMOTE_USER
   REQUEST_METHOD SCRIPT_NAME SERVER_NAME SERVER_PORT SERVER_PROTOCOL SERVER_SOFTWARE].each do |key|
  expected = %w[CONTENT_TYPE CONTENT_LENGTH].include?(key) ? key : "HTTP_" + key
  cases << [key.tr("_", "-"), expected]
  cases << [key.downcase.tr("_", "-"), expected]
  cases << [key, "HTTP_" + key.tr("_", ",")]
end
"!#$%&'*+.^`|~".each_char { |char| cases << ["X#{char}Case", "HTTP_X#{char}CASE"] }

cases.each do |name, expected|
  value = case expected
  when "CONTENT_LENGTH" then "0"
  when "HTTP_HTTPS" then "on"
  when "HTTP_RACK.URL,SCHEME" then "https"
  else "probe"
  end
  raw = "GET /actual HTTP/1.1\r\nHost: example.test\r\n#{name}: #{value}\r\n\r\n"
  req = Tep::Parser.parse(raw)
  raise "parser rejected #{name}" unless req
  request_obj = ActionDispatch::Request.new
  eval(copy_loop, binding, scaffold, start + 1)
  wanted = { "HTTP_HOST" => "example.test", expected => value }
  if defined?(Puma::HttpParser)
    env = {}
    parser = Puma::HttpParser.new
    parser.execute(env, raw.dup, 0)
    raise "Puma rejected #{name}" unless parser.finished?
    actual = env.select { |key, _| key.start_with?("HTTP_") || %w[CONTENT_TYPE CONTENT_LENGTH].include?(key) }
    raise "Puma #{name}: #{actual.inspect} != #{wanted.inspect}" unless actual == wanted
  end
  raise "#{name}: #{request_obj.env.inspect} != #{wanted.inspect}" unless request_obj.env == wanted
  raise "#{name} changed ssl?" if request_obj.ssl?
end

raw = "GET / HTTP/1.1\r\nHost: example.test\r\nHTTPS: on\r\nrack.url_scheme: https\r\nX-Forwarded-Proto: http\r\n\r\n"
req = Tep::Parser.parse(raw)
request_obj = ActionDispatch::Request.new
eval(copy_loop, binding, scaffold, start + 1)
raise "wire server keys changed ssl?" if request_obj.ssl?
puts "#{cases.length} parsed wire headers and scheme isolation pass"
