# Validate every Content-Length before a later field can overwrite it.
require_relative "tep_server_harness"

INVALID = [
  ["1", "2"], ["02", "2"], ["2", "1"], ["nope", "2"], ["2", "nope"],
  ["", "2"], ["2", ""], ["2", "3", "2"], [""], ["2x"],
  ["-2"], ["+2"], ["2, 2"], ["2, 3"],
  ["9" * 25, "8" * 25], ["2\x00"], ["2\r"], ["2\n"],
  ["\v2"], ["2\f"], ["2\x00", "2"]
].freeze

def lengths(values)
  fields = values.each_with_index.map do |value, i|
    "#{i.odd? ? 'cOnTeNt-LeNgTh' : 'Content-Length'}: #{value}\r\n"
  end.join
  "POST /posts HTTP/1.1\r\nHost: localhost\r\n#{fields}\r\n" + "é".b + "x" * 8192
end

SERVERS.each_key do |server|
  invalid_requests = INVALID.map { |values| lengths(values) }
  ["Content-Length : 2\r\nContent-Length: 1\r\n",
   "Content-Length\t: 2\r\nContent-Length: 1\r\n",
   "Content-Length: 2\r\n 0\r\n"].each do |fields|
    invalid_requests << "POST /posts HTTP/1.1\r\nHost: localhost\r\n#{fields}\r\nab"
  end
  ["1.0", "1.1"].each do |version|
    invalid_requests.each do |bytes|
      result = serve(server, bytes.sub("HTTP/1.1", "HTTP/#{version}"))
      status, recvs, bodies, raised, keep_alive = result
      headers = bytes.split("\r\n\r\n").first
      check(
        "#{server}: HTTP/#{version} #{headers.inspect} is 400 before a body drain or dispatch",
        raised.nil? && status == 400 && recvs == 1 && bodies.empty?,
        describe(*result)
      )
      check(
        "#{server}: invalid Content-Length closes the connection",
        raised.nil? && keep_alive == false && Sock.wire.out.include?("Connection: close\r\n"),
        describe(*result)
      )
    end
  end
  ["Content-Length: 2\r\n", "Content-Length: 2\r\ncontent-length: 2\r\n",
   "Content-Length: 0002\r\nContent-Length: 0002\r\n",
   "Content-Length: \t2 \t\r\ncOnTeNt-LeNgTh:\t 2\r\n", ""].each do |fields|
    body = fields.empty? ? "" : "é".b
    result = serve(server, "POST /posts HTTP/1.1\r\nHost: localhost\r\n#{fields}\r\n" + body)
    status, _recvs, bodies, raised, keep_alive = result
    check(
      "#{server}: #{fields.inspect} still serves its complete body",
      raised.nil? && status == 200 && keep_alive && bodies.map(&:b) == [body.b],
      describe(*result)
    )
  end
end

SERVERS.each_key do |server|
  result = serve(server, post(0, ""))
  status, _recvs, bodies, raised, keep_alive = result
  check("#{server}: an explicit zero still serves", raised.nil? && status == 200 && keep_alive && bodies == [""])
end

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
