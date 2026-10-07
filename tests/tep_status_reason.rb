# Exercise the real response writers for all three server modes.
require_relative "tep_server_harness"

def APP.dispatch(req, res)
  res.status = { "/conflict" => 409, "/invalid" => 422, "/created" => 201 }.fetch(req.path)
  res.body = "result"
end

SERVERS.each_key do |server|
  { "conflict" => "409 Conflict", "invalid" => "422 Unprocessable Content", "created" => "201 Created" }.each do |path, expected|
    serve(server, "GET /#{path} HTTP/1.1\r\nHost: localhost\r\n\r\n")
    actual = Sock.wire.out.lines.first.to_s.strip
    raise "#{server}: #{actual.inspect}, expected HTTP/1.1 #{expected}" unless actual == "HTTP/1.1 #{expected}"
  end
end
puts "9 status lines pass"
