require 'minitest/autorun'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'json'
require 'rbconfig'

class RhVerifyTest < Minitest::Test
  SOURCE = File.expand_path('..', __dir__)
  GIT_LOCATION_ENV = %w[GIT_DIR GIT_COMMON_DIR GIT_WORK_TREE GIT_INDEX_FILE
    GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_PREFIX].to_h { |key| [key, nil] }.freeze

  def setup
    @root = Dir.mktmpdir('rh-verify-')
    %w[bin scripts tests src/emit docs/guide fixtures/real-blog fixtures/store mocks].each do |dir|
      FileUtils.mkdir_p(File.join(@root, dir))
    end
    FileUtils.cp(File.join(SOURCE, 'bin/rh'), File.join(@root, 'bin/rh'))
    FileUtils.cp(File.join(SOURCE, 'scripts/ci-plan.py'), File.join(@root, 'scripts/ci-plan.py'))
    File.write(File.join(@root, '.gitignore'), "mocks/\ncargo.log\n")
    File.write(File.join(@root, 'src/emit/go.rs'), 'before')
    %w[example ruby_toolchain].each { |name| File.write(File.join(@root, "tests/#{name}.rs"), '') }
    git('init', '-q')
    git('add', '.')
    git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'base')
    @base = git('rev-parse', 'HEAD').strip
    @log = File.join(@root, 'cargo.log')
    cargo = File.join(@root, 'mocks/cargo')
    File.write(cargo, <<~RUBY)
      #!#{RbConfig.ruby}
      require 'json'
      if ARGV == ['--version']
        puts 'cargo 1.98.1'
        exit 0
      end
      if ARGV.first == 'metadata'
        File.write(File.join(__dir__, 'metadata.log'), JSON.generate(ARGV))
        if ENV['VERIFY_METADATA_FAIL']
          warn 'metadata unavailable'
          exit 9
        end
        puts JSON.generate(target_directory: ENV.fetch('VERIFY_TARGET_DIRECTORY', File.join(Dir.pwd, 'target')))
        exit 0
      end
      File.open(ENV.fetch('VERIFY_LOG'), 'a') do |f|
        f.puts JSON.generate(args: ARGV, cwd: Dir.pwd, jobs: ENV['CARGO_BUILD_JOBS'], debug: ENV['CARGO_PROFILE_TEST_DEBUG'],
          dev_debug: ENV['CARGO_PROFILE_DEV_DEBUG'],
          dev_strip: ENV['CARGO_PROFILE_DEV_STRIP'], test_strip: ENV['CARGO_PROFILE_TEST_STRIP'],
          dev_opt: ENV['CARGO_PROFILE_DEV_OPT_LEVEL'], test_opt: ENV['CARGO_PROFILE_TEST_OPT_LEVEL'],
          git_dir: ENV['GIT_DIR'], git_work_tree: ENV['GIT_WORK_TREE'])
      end
      puts 'child stdout'
      warn 'child stderr'
      exit(ARGV.include?(ENV['VERIFY_FAIL']) ? 17 : 0)
    RUBY
    File.chmod(0o755, cargo)
    df = File.join(@root, 'mocks/df')
    File.write(df, <<~RUBY)
      #!#{RbConfig.ruby}
      if ENV['VERIFY_DF_FAIL']
        warn 'df unavailable'
        exit 3
      end
      if ENV['VERIFY_DF_MALFORMED']
        puts 'unknown disk format'
        exit 0
      end
      abort 'wrong df units or locale' unless ARGV.first(2) == ['-P', '-k'] && ENV['LC_ALL'] == 'C'
      available = ENV.fetch('VERIFY_DF_AVAILABLE_KB', '24000000').to_i
      if ARGV.last == ENV['VERIFY_TARGET_PARENT']
        available = File.exist?(ENV.fetch('VERIFY_LOG')) ? 1_000_000 : 6_000_000
      end
      puts 'Filesystem 1024-blocks Used Available Capacity Mounted on'
      puts "/dev/test volume 90000000 66000000 \#{available} 74% /volume with spaces"
    RUBY
    File.chmod(0o755, df)
  end

  def teardown
    FileUtils.remove_entry(@root)
  end

  def git(*args)
    out, err, status = Open3.capture3(GIT_LOCATION_ENV, 'git', *args, chdir: @root)
    assert status.success?, err
    out
  end

  def invoke(*args, env: {})
    Open3.capture3(GIT_LOCATION_ENV.merge('PATH' => "#{@root}/mocks:#{ENV.fetch('PATH')}", 'VERIFY_LOG' => @log,
      'CARGO_BUILD_JOBS' => nil, 'CARGO_PROFILE_DEV_DEBUG' => nil, 'CARGO_PROFILE_TEST_DEBUG' => nil,
      'CARGO_PROFILE_DEV_STRIP' => nil, 'CARGO_PROFILE_TEST_STRIP' => nil,
      'CARGO_PROFILE_DEV_OPT_LEVEL' => nil, 'CARGO_PROFILE_TEST_OPT_LEVEL' => nil).merge(env),
      RbConfig.ruby, File.join(@root, 'bin/rh'), 'verify', *args, chdir: '/')
  end

  def calls
    File.exist?(@log) ? File.readlines(@log).map { |line| JSON.parse(line) } : []
  end

  def python_available?
    system('python3', '--version', out: File::NULL, err: File::NULL)
  end

  def test_plan_is_read_only_and_routes_committed_dirty_deleted_and_untracked_inputs
    File.write(File.join(@root, 'docs/guide/verify.md'), 'committed guide')
    git('add', 'docs')
    git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'guide')
    File.delete(File.join(@root, 'src/emit/go.rs'))
    FileUtils.mkdir_p(File.join(@root, 'wasm'))
    File.write(File.join(@root, 'wasm/new file.txt'), 'untracked')
    out, err, status = invoke('--plan', '--json', '--base', @base, '--test', 'example')
    assert status.success?, err
    report = JSON.parse(out)
    assert_equal 'planned', report['status']
    assert_equal ['docs/guide/verify.md', 'src/emit/go.rs', 'wasm/new file.txt'], report['changes']
    if python_available?
      assert_includes report['hosted_coverage']['smoke'], 'go'
      assert report['hosted_coverage']['wasm']
      assert_equal false, report['hosted_coverage']['publish']
    else
      assert_nil report['hosted_coverage']
      assert_includes report['hosted_coverage_error'], 'python3'
    end
    assert_equal %w[not-run not-run], report['checks'].map { |c| c['status'] }
    assert_empty calls
    assert_nil report['disk_space']['before']
    assert_nil report['disk_space']['after']
    refute File.exist?(File.join(@root, 'mocks/metadata.log'))
    refute File.exist?(File.join(@root, '.git/rh-verify.lock'))
    refute File.exist?(File.join(@root, 'scripts/__pycache__'))
  end

  def test_inherited_git_locations_cannot_redirect_reports_locks_or_checks
    Dir.mktmpdir('rh-foreign-') do |foreign|
      git('-C', foreign, 'init', '-q')
      File.write(File.join(foreign, 'foreign.txt'), 'foreign')
      git('-C', foreign, 'add', '.')
      git('-C', foreign, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'foreign')
      File.write(File.join(@root, 'src/emit/go.rs'), 'dirty local source')
      foreign_git = File.join(foreign, '.git')
      env = { 'GIT_DIR' => foreign_git, 'GIT_COMMON_DIR' => foreign_git, 'GIT_WORK_TREE' => foreign,
        'GIT_INDEX_FILE' => File.join(foreign_git, 'index'), 'GIT_OBJECT_DIRECTORY' => File.join(foreign_git, 'objects'),
        'GIT_ALTERNATE_OBJECT_DIRECTORIES' => File.join(foreign_git, 'objects'), 'GIT_PREFIX' => 'foreign/' }
      out, err, status = invoke('--json', env: env)
      assert status.success?, err
      report = JSON.parse(out)
      assert_equal @base, report['head']
      assert_equal ['src/emit/go.rs'], report['changes']
      assert_equal 'passed', report['status']
      assert File.exist?(File.join(@root, '.git/rh-verify.lock'))
      refute File.exist?(File.join(foreign_git, 'rh-verify.lock'))
      assert_equal 1, calls.length
      assert_nil calls.first['git_dir']
      assert_nil calls.first['git_work_tree']
    end
  end

  def test_success_executes_exact_commands_in_order_and_keeps_json_clean
    out, err, status = invoke('--json', '--jobs', '2', '--test', 'example', '--test', 'example', '--toolchain', 'ruby')
    assert status.success?, err
    report = JSON.parse(out)
    assert_equal 'passed', report['status']
    assert_equal [
      %w[test --locked --lib -- --test-threads=1],
      %w[test --locked --test example -- --test-threads=1],
      %w[test --locked --test ruby_toolchain -- --ignored --test-threads=1]
    ], calls.map { |c| c['args'] }
    assert calls.all? { |c| c['cwd'] == @root && c['jobs'] == '2' && c['debug'].nil? && c['dev_debug'].nil? }
    assert report['checks'].all? { |c| c['status'] == 'passed' && c['exit'] == 0 && c['seconds'] >= 0 }
    assert_includes err, 'child stdout'
    assert_includes report['scope'], 'not executed'
  end

  def test_ignored_integration_selection_does_not_expand_library_or_native_scope
    out, err, status = invoke('--json', '--ignored', '--test', 'example', '--toolchain', 'ruby')
    assert status.success?, err
    assert_equal [
      %w[test --locked --lib -- --test-threads=1],
      %w[test --locked --test example -- --ignored --test-threads=1],
      %w[test --locked --test ruby_toolchain -- --ignored --test-threads=1]
    ], calls.map { |c| c['args'] }
    assert_equal 'passed', JSON.parse(out)['status']
  end

  def test_failure_stops_execution_preserves_exit_and_leaves_remaining_check_unrun
    out, _err, status = invoke('--json', '--test', 'example', env: { 'VERIFY_FAIL' => '--lib' })
    assert_equal 17, status.exitstatus
    report = JSON.parse(out)
    assert_equal 'failed', report['status']
    assert_equal %w[failed not-run], report['checks'].map { |c| c['status'] }
    assert_equal 1, calls.length
    assert_equal 24_000_000 * 1024, report['disk_space']['after']['workspace']['available_bytes']
  end

  def test_explicit_environment_is_respected
    out, err, status = invoke('--json', env: { 'CARGO_BUILD_JOBS' => '3', 'CARGO_PROFILE_TEST_DEBUG' => '1' })
    assert status.success?, err
    assert_equal '3', JSON.parse(out)['build_environment']['CARGO_BUILD_JOBS']
    assert calls.all? { |c| c['jobs'] == '3' && c['debug'] == '1' }
  end

  def test_optional_profile_settings_are_reported_without_changing_defaults
    settings = {
      'CARGO_PROFILE_DEV_DEBUG' => '2', 'CARGO_PROFILE_TEST_DEBUG' => 'line-tables-only',
      'CARGO_PROFILE_DEV_STRIP' => 'none', 'CARGO_PROFILE_TEST_STRIP' => 'symbols',
      'CARGO_PROFILE_DEV_OPT_LEVEL' => '2', 'CARGO_PROFILE_TEST_OPT_LEVEL' => '1'
    }
    out, err, status = invoke('--plan', '--json')
    assert status.success?, err
    settings.each_key { |key| refute JSON.parse(out)['build_environment'].key?(key) }
    out, err, status = invoke('--json', env: settings)
    assert status.success?, err
    settings.each { |key, value| assert_equal value, JSON.parse(out)['build_environment'][key] }
    assert_equal ['2', 'line-tables-only', 'none', 'symbols', '2', '1'],
      calls.first.values_at('dev_debug', 'debug', 'dev_strip', 'test_strip', 'dev_opt', 'test_opt')
  end

  def test_missing_fixtures_are_reported_without_blocking_unrelated_checks
    FileUtils.remove_entry(File.join(@root, 'fixtures/store'))
    out, err, status = invoke('--json')
    assert status.success?, err
    assert_equal 'passed', JSON.parse(out)['status']
    assert_equal ['store'], JSON.parse(out)['missing_fixtures']
    assert_equal 1, calls.length
    out, err, status = invoke('--plan', '--json')
    assert status.success?, err
    assert_equal 'planned', JSON.parse(out)['status']
    assert_equal 1, calls.length
  end

  def test_informational_policy_failure_does_not_block_local_execution
    File.write(File.join(@root, 'scripts/ci-plan.py'), "raise RuntimeError('unavailable policy')\n")
    out, err, status = invoke('--json')
    assert status.success?, err
    report = JSON.parse(out)
    assert_equal 'passed', report['status']
    assert_nil report['hosted_coverage']
    assert_includes report['hosted_coverage_error'], python_available? ? 'unavailable policy' : 'python3'
    assert_equal 1, calls.length
  end

  def test_changed_policy_format_remains_informational
    File.write(File.join(@root, 'scripts/ci-plan.py'), "def select(paths): return []\n")
    out, err, status = invoke
    assert status.success?, err
    assert_includes out, 'Hosted selection unavailable:'
    assert_includes out, python_available? ? 'unsupported hosted coverage format' : 'python3'
    assert_includes out, 'Local checks: passed'
    assert_equal 1, calls.length
  end

  def test_disk_snapshots_resolve_cargo_target_and_probe_existing_parent_without_creating_it
    parent = File.join(@root, 'external cache')
    target = File.join(parent, 'not created', 'target')
    FileUtils.mkdir_p(parent)
    out, err, status = invoke('--json', env: { 'VERIFY_TARGET_DIRECTORY' => target, 'VERIFY_TARGET_PARENT' => parent })
    assert status.success?, err
    report = JSON.parse(out)
    disk = report['disk_space']
    assert_equal 5 * 1024**3, disk['warning_threshold_bytes']
    assert_equal 24_000_000 * 1024, disk['before']['workspace']['available_bytes']
    assert_equal 90_000_000 * 1024, disk['before']['workspace']['total_bytes']
    assert_equal 6_000_000 * 1024, disk['before']['cargo_target']['available_bytes']
    assert_equal 1_000_000 * 1024, disk['after']['cargo_target']['available_bytes']
    assert_equal target, disk['before']['cargo_target']['path']
    assert_equal parent, disk['before']['cargo_target']['filesystem_path']
    assert_includes err, 'less than 5 GiB'
    assert_includes err, target
    refute_includes err, 'available before'
    assert_equal 'passed', report['status']
    assert_equal %w[metadata --format-version 1 --no-deps --offline --locked],
      JSON.parse(File.read(File.join(@root, 'mocks/metadata.log')))
    refute File.exist?(target)
  end

  def test_optional_disk_failures_never_block_checks_or_hide_cargo_failure
    [{ 'VERIFY_DF_FAIL' => '1' }, { 'VERIFY_DF_MALFORMED' => '1' }].each do |env|
      out, err, status = invoke('--json', env: env)
      assert status.success?, err
      report = JSON.parse(out)
      assert_equal 'passed', report['status']
      assert report['disk_space']['before']['workspace']['error']
      assert report['disk_space']['after']['cargo_target']['error']
    end
    out, _err, status = invoke('--json', env: { 'VERIFY_METADATA_FAIL' => '1', 'VERIFY_FAIL' => '--lib' })
    assert_equal 17, status.exitstatus
    report = JSON.parse(out)
    assert_equal 'failed', report['status']
    assert_includes report['disk_space']['cargo_target_error'], 'metadata unavailable'
    assert_nil report['disk_space']['before']['cargo_target']
    assert_equal 24_000_000 * 1024, report['disk_space']['after']['workspace']['available_bytes']
  end

  def test_low_space_warning_boundary_does_not_change_test_results
    [5_242_880, 5_242_879].each do |available_kb|
      out, err, status = invoke('--json', env: { 'VERIFY_DF_AVAILABLE_KB' => available_kb.to_s })
      assert status.success?, err
      report = JSON.parse(out)
      assert_equal available_kb * 1024, report['disk_space']['before']['workspace']['available_bytes']
      assert_equal available_kb < 5_242_880, err.include?('less than 5 GiB')
      assert_equal 'passed', report['status']
    end
  end

  def test_missing_df_and_cargo_remain_optional_probe_errors_and_real_check_failures
    git_path = ENV.fetch('PATH').split(File::PATH_SEPARATOR).map { |dir| File.join(dir, 'git') }
      .find { |path| File.executable?(path) && !File.directory?(path) }
    File.symlink(git_path, File.join(@root, 'mocks/git'))
    File.delete(File.join(@root, 'mocks/df'))
    out, err, status = invoke('--json', env: { 'PATH' => File.join(@root, 'mocks') })
    assert status.success?, err
    report = JSON.parse(out)
    assert_equal 'passed', report['status']
    assert_includes report['disk_space']['before']['workspace']['error'], 'df'
    assert_nil report['hosted_coverage']
    assert_includes report['hosted_coverage_error'], 'python3'
    out, err, status = Open3.capture3(GIT_LOCATION_ENV.merge('PATH' => File.join(@root, 'mocks')),
      RbConfig.ruby, File.join(@root, 'bin/rh'), 'doctor')
    assert status.success?, err
    assert_match(/^  bin\/rh verify$/, out)
    assert_includes out, 'optional verify hosted-coverage preview'
    assert_includes out, 'Install to unlock more:'
    refute_includes out.split('Install to unlock more:').last, 'Python'
    File.delete(File.join(@root, 'mocks/cargo'))
    out, _err, status = invoke('--json', env: { 'PATH' => File.join(@root, 'mocks') })
    assert_equal 127, status.exitstatus
    report = JSON.parse(out)
    assert_equal 'failed', report['status']
    assert_includes report['disk_space']['cargo_target_error'], 'cargo'
    assert_equal 127, report['checks'].first['exit']
  end

  def test_help_human_results_and_doctor_discover_verification
    out, err, status = invoke('--help')
    assert status.success?, err
    assert_includes out, '--plan'
    assert_empty calls
    out, err, status = invoke
    assert status.success?, err
    assert_includes out, 'Local checks: passed'
    assert_includes out, 'not hosted CI'
    assert_includes out, 'Disk before (workspace)'
    assert_includes out, 'Disk after (cargo_target)'
    out, _err, status = invoke(env: { 'VERIFY_FAIL' => '--lib' })
    assert_equal 17, status.exitstatus
    assert_includes out, 'Local checks: failed'
    out, err, status = Open3.capture3({ 'PATH' => "#{@root}/mocks:#{ENV.fetch('PATH')}", 'VERIFY_LOG' => @log },
      RbConfig.ruby, File.join(@root, 'bin/rh'), 'doctor')
    assert status.success?, err
    assert_includes out, 'bin/rh verify --plan'
    assert_includes out, 'bin/rh verify'
    assert_includes out, 'optional verify hosted-coverage preview'
  end

  def test_invalid_inputs_never_start_cargo
    [%w[--test ../example], %w[--toolchain unknown], %w[--jobs 0],
     %w[--base nonexistent], %w[--base --help], %w[--ignored], %w[extra]].each do |args|
      _out, err, status = invoke(*args)
      assert_equal 2, status.exitstatus, args.inspect
      assert_includes err, 'rh verify:'
    end
    assert_empty calls
  end

  def test_concurrent_run_is_rejected
    File.open(File.join(@root, '.git/rh-verify.lock'), 'w') do |lock|
      lock.flock(File::LOCK_EX)
      _out, err, status = invoke('--json')
      assert_equal 2, status.exitstatus
      assert_includes err, 'another verify run'
      assert_empty calls
      refute File.exist?(File.join(@root, 'mocks/metadata.log'))
    end
  end
end
