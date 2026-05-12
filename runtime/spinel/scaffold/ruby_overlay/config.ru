# Rack adapter for the CRuby target.
#
# Puma loads this file, mounts the returned app, and serves HTTP/1.1.
# Each request goes Rack env → CGI-shaped env → Main.run → CGI-shaped
# response bytes → Rack tuple. Main.run is target-portable: the same
# dispatch code runs here under CRuby and (once sphttp lands) inside
# the spinel-compiled binary.
#
# The CGI shape is an interim transport; both this Rack adapter and
# the eventual sphttp accept loop construct identical CGI env hashes
# before calling Main.run. When Main.run gets reshaped to a more
# direct Rack-compatible interface, this file shrinks accordingly.

require "stringio"
require "rack"
require_relative "main"

Main.configure_default_adapter!

# Static asset serving — `rake assets` lays files out under
# `static/assets/*` (CSS + JS) plus root-level icons. Rack::Static
# intercepts those URLs before the Rack adapter dispatches to
# Main.run. Anything else falls through to the dynamic Router.
use Rack::Static, urls: ["/assets", "/icon.png", "/icon.svg"], root: "static"

# Convert Rack env → CGI env shape Main.run expects.
RACK_TO_CGI = {
  "REQUEST_METHOD" => "REQUEST_METHOD",
  "PATH_INFO"      => "PATH_INFO",
  "QUERY_STRING"   => "QUERY_STRING",
  "CONTENT_LENGTH" => "CONTENT_LENGTH",
  "CONTENT_TYPE"   => "CONTENT_TYPE",
  "HTTP_COOKIE"    => "HTTP_COOKIE",
}.freeze

# Parse Main.run's CGI-style response ("Status: 200 OK\r\nContent-Type: ...\r\n\r\nbody")
# back into a Rack response tuple. Multi-value headers (Set-Cookie can
# repeat) are coalesced into arrays per Rack 3.x spec.
def parse_cgi_response(raw)
  head, body = raw.split("\r\n\r\n", 2)
  body ||= ""
  status = 200
  headers = {}
  head.to_s.each_line(chomp: true) do |line|
    if line =~ /\AStatus:\s*(\d+)/
      status = ::Regexp.last_match(1).to_i
    elsif line =~ /\A([^:]+):\s*(.*)\z/
      name, value = ::Regexp.last_match(1), ::Regexp.last_match(2)
      if headers.key?(name)
        existing = headers[name]
        headers[name] = existing.is_a?(Array) ? (existing << value) : [existing, value]
      else
        headers[name] = value
      end
    end
  end
  [status, headers, [body]]
end

app = lambda do |env|
  cgi_env = {}
  RACK_TO_CGI.each { |rack_key, cgi_key| cgi_env[cgi_key] = env[rack_key].to_s }
  stdin = env["rack.input"] || StringIO.new("")
  stdout = StringIO.new
  Main.run(cgi_env, stdin, stdout)
  parse_cgi_response(stdout.string)
end

run app
