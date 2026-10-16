# frozen_string_literal: true
# The key signed messages derive from when SECRET_KEY_BASE is unset
# (runtime/local_secret.rb): generated once, kept owner-only in storage/,
# read back on the next boot, and never the empty string.
require "minitest/autorun"
require "tmpdir"
require_relative "../runtime/local_secret"

class LocalSecretTest < Minitest::Test
  def in_tmp
    Dir.mktmpdir { |dir| Dir.chdir(dir) { yield } }
  end

  def test_the_environment_wins_and_nothing_is_written
    in_tmp do
      assert_equal "from-env", LocalSecret.resolve("from-env")
      refute File.exist?(LocalSecret::PATH)
    end
  end

  def test_unset_generates_a_key_and_keeps_it_owner_only
    in_tmp do
      key = LocalSecret.resolve(nil)
      assert_match(/\A[0-9a-f]{128}\z/, key)
      assert_equal key, File.read(LocalSecret::PATH).strip
      assert_equal 0o600, File.stat(LocalSecret::PATH).mode & 0o777
    end
  end

  def test_the_next_boot_reads_the_same_key
    in_tmp do
      first = LocalSecret.resolve("")
      assert_equal first, LocalSecret.resolve(nil)
    end
  end

  def test_two_deployments_get_different_keys
    a = in_tmp { LocalSecret.resolve(nil) }
    b = in_tmp { LocalSecret.resolve(nil) }
    refute_equal a, b
  end
end
