# Isolated Ractor checks for the libraries a roundhouse CRuby emit loads.
#
#   cd EMIT && bundle exec ruby /path/to/scripts/ractor-probe-calls.rb [--no-freeze]
#
# Each case is one representative call, run once in the main Ractor and
# once in a fresh worker Ractor. Before the worker runs, every constant
# that can be made shareable is (the boot-time deep freeze Tobi's
# campfire-once-ruby-ractor port does in lib/ractor_compat.rb), so a FAIL
# here is NOT "this constant is unfrozen" — it is a C extension that
# does not declare Ractor safety, a mutable registry, a Mutex, or a Proc.
# `--no-freeze` shows what fails without that pass.
#
# Feeds docs/ractor-ledger.md.

$VERBOSE = nil
require "stringio"
%w[sqlite3 nokogiri loofah rails-html-sanitizer digest/md5 digest/sha1 io/wait
   websocket/driver bcrypt openssl zlib json concurrent net/http/persistent
   connection_pool securerandom base64 rack rqrcode sentry-ruby resolv net/http
   socket].each { |lib| require lib }
# Autoloaded constants load after a boot-time freeze unless forced first.
Rack::Files; Rack::Utils; Rack::Request; Rack::Response

module Cases
  PUBLIC = File.expand_path("public").freeze

  def self.sqlite3 = SQLite3::Database.new(":memory:").execute("select 1")
  def self.nokogiri_html5 = Nokogiri::HTML5.fragment("<p>hi <b>x</b></p>").to_html
  def self.nokogiri_html4 = Nokogiri::HTML4::DocumentFragment.parse("<p>hi</p>").to_html
  def self.nokogiri_xml = Nokogiri::XML("<a><b/></a>").root.name
  def self.loofah = Loofah.html5_fragment("<p onclick=x>hi<script>y</script></p>").scrub!(:prune).to_s
  def self.sanitizer = Rails::HTML5::SafeListSanitizer.new.sanitize("<p onclick=x>hi</p>")
  def self.websocket_mask = WebSocket::Mask.mask("abcd".b, [1, 2, 3, 4].pack("C*"))
  def self.digest_md5 = Digest::MD5.hexdigest("x")
  def self.digest_sha1 = Digest::SHA1.hexdigest("x")
  def self.io_wait = (r, w = IO.pipe; w.write("x"); r.wait_readable(0) ? :ok : :none)
  def self.transcode = "café".encode("ISO-8859-1").encode("UTF-8")
  def self.bcrypt = BCrypt::Password.create("x", cost: 4).is_password?("x")
  def self.openssl_hmac = OpenSSL::HMAC.hexdigest("SHA256", "k", "d")
  def self.openssl_digest_new = OpenSSL::Digest.new("SHA256").update("d").hexdigest
  def self.openssl_digest_sha256 = OpenSSL::Digest::SHA256.new.update("d").hexdigest
  def self.openssl_ec = OpenSSL::PKey::EC.generate("prime256v1").public_key.to_bn.to_s(2).bytesize
  def self.zlib = Zlib::Deflate.deflate("x" * 100).bytesize
  def self.json = JSON.parse(JSON.generate({ "a" => [1] }))
  def self.securerandom = [SecureRandom.hex(8), SecureRandom.uuid]
  def self.base64 = Base64.urlsafe_decode64(Base64.urlsafe_encode64("x"))
  def self.concurrent_pool = (p = Concurrent::FixedThreadPool.new(1); p.post { 1 }; p.shutdown; p.wait_for_termination(2))
  def self.concurrent_map = Concurrent::Map.new.tap { |m| m[:a] = 1 }[:a]
  def self.connection_pool = ConnectionPool.new(size: 1) { Object.new }.with(&:class)
  def self.net_http_persistent = Net::HTTP::Persistent.new(name: "probe", pool_size: 2).shutdown
  def self.rack_files = Rack::Files.new(PUBLIC).call({ "REQUEST_METHOD" => "GET", "PATH_INFO" => "/robots.txt",
    "SCRIPT_NAME" => "", "QUERY_STRING" => "", "SERVER_NAME" => "x", "SERVER_PORT" => "80", "rack.input" => StringIO.new })[0]
  def self.rqrcode = RQRCode::QRCode.new("https://example.com").as_svg.bytesize
  def self.sentry = Sentry.initialized?
  def self.resolv = Resolv.getaddresses("localhost")
  def self.getaddrinfo = Addrinfo.getaddrinfo("localhost", 80).size
  def self.net_http
    server = TCPServer.new("127.0.0.1", 0)
    t = Thread.new { c = server.accept; c.readpartial(1000); c.write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"); c.close }
    code = Net::HTTP.get_response(URI("http://127.0.0.1:#{server.addr[1]}/")).code
    t.join
    code
  end
end

unless ARGV.include?("--no-freeze")
  ObjectSpace.each_object(Module) do |mod|
    next if mod == Cases
    mod.constants(false).each do |c|
      next if mod.autoload?(c)
      v = (mod.const_get(c, false) rescue next)
      next if v.is_a?(Module) || v.is_a?(IO) || v.equal?(ENV) || v.equal?(ARGF) || Ractor.shareable?(v)
      Ractor.make_shareable(v) rescue nil
    end
  end
end

Cases.singleton_methods(false).each do |m|
  main = begin; Cases.send(m); "ok"; rescue Exception => e; "FAILS TOO: #{e.class}"; end
  v = Ractor.new(m) do |mm|
    Cases.send(mm)
    [:ok]
  rescue Exception => ex
    [ex.class.to_s, ex.message[0, 140], (ex.backtrace || []).first.to_s.sub(%r{.*/gems/}, "")]
  end.value
  line = v[0] == :ok ? "OK    #{m}" : "FAIL  #{m} -> #{v[0]}: #{v[1]}  @ #{v[2]}"
  puts "#{line}  (main Ractor: #{main})"
end
