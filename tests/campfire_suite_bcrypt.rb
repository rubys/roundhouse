# Exercise the real suite launcher, not just the bcrypt preload in isolation.
require "tmpdir"
require "open3"
require "rbconfig"

suite = File.expand_path("../scripts/campfire-suite", __dir__)
Dir.mktmpdir("roundhouse-campfire-suite-bcrypt") do |emit|
  Dir.mkdir(File.join(emit, "test"))
  File.write(File.join(emit, "Makefile"), "SPINEL_TESTS := test/bcrypt_test\n\n")
  File.write(File.join(emit, "test/bcrypt_test.rb"), <<~RUBY)
    require "bcrypt"
    class BcryptTest
      def test_configuration
        raise "main script changed" unless $0 == "test/bcrypt_test.rb"
        raise "arguments changed" unless ARGV.empty?
        raise "working directory changed" unless Dir.pwd == ENV.fetch("SUITE_TEST_ROOT")
        raise "production default changed" unless BCrypt::Engine::DEFAULT_COST == 12
        digest = BCrypt::Password.create("correct horse")
        explicit = BCrypt::Password.create("correct horse", cost: 6)
        raise "test cost not applied" unless digest.cost == 4
        raise "explicit cost changed" unless explicit.cost == 6
        raise "authentication changed" unless digest == "correct horse" && explicit == "correct horse"
        raise "wrong password accepted" if digest == "wrong horse" || explicit == "wrong horse"
        raise "expected test failure" if ENV["FAIL_SUITE_TEST"] == "1"
      end
    end
    __t = BcryptTest.new
    begin
      __t.test_configuration
    end
    puts "BcryptTest: 1 tests passed"
  RUBY

  [false, true].each do |fail|
    tally = File.join(emit, "tally.txt")
    stdout, stderr, status = Open3.capture3(
      { "SUITE_TEST_ROOT" => emit, "FAIL_SUITE_TEST" => fail ? "1" : "0" },
      "bash", suite, "--reuse", emit, "--tally", tally
    )
    raise "suite failed: #{stdout}\n#{stderr}" unless status.success?
    expected = fail ? "FAIL|test/bcrypt_test|0|1|" : "PASS|test/bcrypt_test|1|1|"
    row = File.read(tally)
    raise "wrong tally: #{row}\n#{stdout}\n#{stderr}" unless row.start_with?(expected)
    raise "wrong failure: #{row}" if fail && !row.include?("expected test failure")
  end
end

_, stderr, status = Open3.capture3(
  RbConfig.ruby, "-rbcrypt", "-e",
  "raise 'ordinary process changed' unless BCrypt::Engine.cost == BCrypt::Engine::DEFAULT_COST"
)
raise stderr unless status.success?
puts "campfire-suite bcrypt regression: hashing, launcher and pass/failure tallies OK"
