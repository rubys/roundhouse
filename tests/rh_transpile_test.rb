require 'minitest/autorun'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'json'
require 'rbconfig'

# `bin/rh transpile` is what scripts/compare emits with. CI stages
# build-roundhouse's binary as ROUNDHOUSE_BIN so the job does not compile a
# release Roundhouse of its own; locally it still builds through Cargo.
class RhTranspileTest < Minitest::Test
  SOURCE = File.expand_path('..', __dir__)

  def setup
    @root = File.realpath(Dir.mktmpdir('rh-transpile-'))
    %w[bin mocks fixtures/real-blog/storage].each { |dir| FileUtils.mkdir_p(File.join(@root, dir)) }
    FileUtils.cp(File.join(SOURCE, 'bin/rh'), File.join(@root, 'bin/rh'))
    FileUtils.cp(File.join(SOURCE, '.ruby-version'), File.join(@root, '.ruby-version'))
    File.write(File.join(@root, 'fixtures/real-blog/storage/development.sqlite3'), 'seed')
    @log = File.join(@root, 'calls.log')
    write_executable('mocks/cargo', <<~RUBY)
      #!#{RbConfig.ruby}
      if ARGV == ['--version']
        puts 'cargo 1.98.1'
        exit 0
      end
      File.open(#{@log.inspect}, 'a') { |f| f.puts JSON.generate(program: 'cargo', args: ARGV) }
      out = ARGV[ARGV.index('-o') + 1]
      FileUtils.mkdir_p(out)
    RUBY
    @compiler = write_executable('mocks/roundhouse', <<~RUBY)
      #!#{RbConfig.ruby}
      require 'json'
      require 'fileutils'
      File.open(#{@log.inspect}, 'a') { |f| f.puts JSON.generate(program: 'roundhouse', args: ARGV) }
      FileUtils.mkdir_p(ARGV[ARGV.index('-o') + 1])
    RUBY
  end

  def teardown
    FileUtils.rm_rf(@root)
  end

  def write_executable(path, body)
    full = File.join(@root, path)
    File.write(full, body.sub("#!#{RbConfig.ruby}\n", "#!#{RbConfig.ruby}\nrequire 'json'\nrequire 'fileutils'\n"))
    File.chmod(0o755, full)
    full
  end

  def transpile(env)
    path = "#{File.join(@root, 'mocks')}:#{ENV.fetch('PATH')}"
    Open3.capture3({ 'PATH' => path }.merge(env), RbConfig.ruby, File.join(@root, 'bin/rh'), 'transpile', 'ruby')
  end

  def calls
    File.exist?(@log) ? File.readlines(@log).map { |line| JSON.parse(line) } : []
  end

  def test_staged_compiler_is_used_and_cargo_is_not
    _out, err, status = transpile('ROUNDHOUSE_BIN' => @compiler, 'ROUNDHOUSE_BIN_TRACE' => '1')
    assert status.success?, err
    out = File.join(@root, 'build/transpiled-blog-ruby')
    assert_equal [{ 'program' => 'roundhouse',
                    'args' => ['--target', 'ruby', '-o', out, File.join(@root, 'fixtures/real-blog')] }], calls
    assert_includes err, "roundhouse-bin: exec #{@compiler}"
    assert_equal 'seed', File.read(File.join(out, 'storage/development.sqlite3'))
  end

  def test_a_relative_staged_compiler_resolves_against_the_callers_directory
    Dir.chdir(@root) do
      _out, err, status = transpile('ROUNDHOUSE_BIN' => 'mocks/roundhouse')
      assert status.success?, err
    end
    assert_equal ['roundhouse'], calls.map { |call| call['program'] }
  end

  def test_a_missing_staged_compiler_fails_instead_of_rebuilding
    _out, err, status = transpile('ROUNDHOUSE_BIN' => File.join(@root, 'mocks/absent'))
    assert_equal 127, status.exitstatus
    assert_includes err, 'ROUNDHOUSE_BIN is not an executable file'
    assert_empty calls
    refute File.exist?(File.join(@root, 'build/transpiled-blog-ruby'))
  end

  def test_without_a_staged_compiler_it_builds_release_through_cargo
    _out, err, status = transpile('ROUNDHOUSE_BIN' => nil)
    assert status.success?, err
    assert_equal 'cargo', calls.first['program']
    assert_equal %w[run --release --quiet --bin roundhouse -- --target ruby], calls.first['args'].first(8)
  end
end
