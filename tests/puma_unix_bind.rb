require "minitest/autorun"
require "tmpdir"

VERIFY_PUMA = ARGV.delete("--puma")
if VERIFY_PUMA
  gem "puma", "8.0.2"
  require "puma"
  require "puma/binder"
end

class PumaUnixBindTest < Minitest::Test
  CONFIG = File.expand_path("../runtime/spinel/scaffold/ruby_overlay/config/puma.rb", __dir__)

  class Config
    attr_reader :binds

    def initialize = @binds = []
    def bind(url) = @binds << url
    def threads(*) = nil
    def port(*) = nil
    def environment(*) = nil
    def workers(*) = nil
    def preload_app! = nil
    def before_worker_boot = nil
    def plugin(*) = nil
  end

  def bind_arguments(url)
    previous = ENV["UNIX_BIND"]
    url.nil? ? ENV.delete("UNIX_BIND") : ENV["UNIX_BIND"] = url
    Config.new.tap { |config| config.instance_eval(File.read(CONFIG), CONFIG) }.binds
  ensure
    previous.nil? ? ENV.delete("UNIX_BIND") : ENV["UNIX_BIND"] = previous
  end

  def test_unset_or_empty_disables_the_unix_listener
    [nil, ""].each { |url| assert_empty(bind_arguments(url)) }
  end

  def test_no_query_gets_the_default_mode
    assert_equal(["unix:///tmp/http.sock?mode=600"], bind_arguments("unix:///tmp/http.sock"))
  end

  def test_explicit_modes_are_passed_through
    %w[0 0000 600 660 0777].each do |mode|
      url = "unix:///tmp/http.sock?mode=#{mode}"
      assert_equal([url], bind_arguments(url))
    end
  end

  def test_invalid_urls_are_refused
    urls = %w[
      unix:///tmp/http.sock?backlog=512
      http:///tmp/http.sock unix://host/tmp/http.sock unix://relative/http.sock
      unix:///tmp/http.sock? unix:///tmp/http.sock?mode=888
      unix:///tmp/http.sock?mode=1000 unix:///tmp/http.sock?mode=00000
      unix:///tmp/http.sock?mode=600&backlog=512
      unix:///tmp/http.sock?mode=600?mode=660
      unix:///tmp/http.sock#fragment unix:///tmp/http.sock?mode=600#fragment
    ]
    urls << "unix:///tmp/http\0.sock"
    urls.each { |url| assert_raises(ArgumentError, url.inspect) { bind_arguments(url) } }
  end

  if VERIFY_PUMA
    def test_real_puma_socket_modes
      { "" => 0600, "?mode=660" => 0660 }.each do |query, mode|
        Dir.mktmpdir("puma-unix-", "/tmp") do |dir|
          path = File.join(dir, "http.sock")
          options = { max_threads: 1, workers: 0, rack_url_scheme: "http" }
          binder = Puma::Binder.new(Puma::LogWriter.strings, options)
          begin
            binder.parse(bind_arguments("unix://#{path}#{query}"))
            assert(File.socket?(path))
            assert_equal(mode, File.stat(path).mode & 0777)
          ensure
            binder.close
          end
        end
      end
    end
  end
end
