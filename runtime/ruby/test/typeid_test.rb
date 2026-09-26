# The `typeid` port, against the gem's own answers.
#
# A FRAMEWORK test (`tests/runtime_ruby_unit.rs` runs every file under
# `runtime/ruby/test/` on each `cargo test`), not one that ships into an
# emitted tree — the CRuby/JRuby trees replace `runtime/typeid.rb` with
# `require "typeid"` (`project::ruby_family_runtime_files`). Here it is
# unambiguously the port.
#
# WHY THIS FILE EXISTS. A wrong suffix does not raise: it is a token that
# still looks like a token, and lobsters would store it. The expectations
# are the gem's (typeid 0.2.2, uuid7 0.2.0), generated with
# `SecureRandom.gen_random` stubbed to the ten bytes below:
#
#     rnd = [0xde,0xad,0xbe,0xef,0x01,0x23,0x45,0x67,0x89,0xab].pack("C*")
#     SecureRandom.define_singleton_method(:gen_random) { |n| rnd }
#     u = TypeID::UUID.generate(timestamp: ts); p [u.bytes, u.base32]
#     p TypeID::UUID::Base32.encode(bytes)
#
# The encode cases cover the two ends of the alphabet, a byte ramp (every
# one of the gem's hand-unrolled masks sees a different bit pattern), and
# the gem's own README example. The generate cases pin the UUIDv7 layout:
# where the version nibble and the variant bits land among the random ones.

require_relative "test_helper"
require "securerandom"
require_relative "../typeid"

class TypeIDTest < Minitest::Test
  ENCODE = {
    [0] * 16 => "00000000000000000000000000",
    [255] * 16 => "7zzzzzzzzzzzzzzzzzzzzzzzzz",
    (0..15).to_a => "00041061050r3gg28a1c60t3gf",
    [0x01, 0x89, 0x37, 0x26, 0xef, 0xee, 0x7f, 0x02, 0x8f, 0xbf, 0x9f, 0x2b, 0x7b, 0xc2, 0xf9, 0x10] =>
      "01h4vjdvzefw18zfwz5dxw5y8g",
  }.freeze

  RANDOM_HEX = "deadbeef0123456789ab"

  GENERATE = {
    1688847445998 => [[1, 137, 55, 38, 239, 238, 126, 173, 190, 239, 1, 35, 69, 103, 137, 171],
                      "01h4vjdvzeftpvxvr14d2pf2db"],
    1790368826744 => [[1, 160, 218, 76, 69, 120, 126, 173, 190, 239, 1, 35, 69, 103, 137, 171],
                      "01m3d4rhbrftpvxvr14d2pf2db"],
  }.freeze

  def test_encode_matches_the_gem
    ENCODE.each { |bytes, want| assert_equal want, TypeID.encode(bytes), bytes.inspect }
  end

  def test_uuid7_layout_matches_the_gem
    GENERATE.each do |ts, (bytes, suffix)|
      got = TypeID.uuid7_bytes(ts, RANDOM_HEX)
      assert_equal bytes, got, "bytes at #{ts}"
      assert_equal suffix, TypeID.encode(got), "suffix at #{ts}"
    end
  end

  def test_new_is_prefix_underscore_suffix
    id = TypeID.new("user")
    assert_kind_of String, id
    assert_match(/\Auser_[0-7][0-9a-hjkmnp-tv-z]{25}\z/, id)
    # Two calls in one millisecond still differ: 74 of the bits are random.
    refute_equal TypeID.new("user"), TypeID.new("user")
  end

  def test_new_timestamp_is_now
    before = (Time.now.to_f * 1000).to_i
    suffix = TypeID.new("x").split("_", 2)[1]
    ms = 0
    TypeID.decode_for_test(suffix).first(6).each { |b| ms = (ms << 8) | b }
    after = (Time.now.to_f * 1000).to_i
    assert_operator ms, :>=, before - 1
    assert_operator ms, :<=, after + 1
  end

  def test_empty_prefix_is_the_bare_suffix
    assert_match(/\A[0-7][0-9a-hjkmnp-tv-z]{25}\z/, TypeID.new(""))
  end

  # The gem's rules and messages, in its order.
  def test_prefix_validation_matches_the_gem
    {
      "User" => "prefix must be lowercase ASCII characters",
      "ab1" => "prefix must be lowercase ASCII characters",
      "_x" => "prefix cannot start or end with an underscore",
      "x_" => "prefix cannot start or end with an underscore",
      "a" * 64 => "prefix length cannot be greater than 63",
    }.each do |prefix, message|
      err = assert_raises(TypeID::Error) { TypeID.new(prefix) }
      assert_equal message, err.message, prefix
    end
    assert_match(/\Ahat_request_/, TypeID.new("hat_request"))
  end
end

# The gem's decoder, test-only: the port has no `from_string`, and the
# timestamp check needs the bytes back.
module TypeID
  def self.decode_for_test(s)
    acc = 0
    s.each_char { |c| acc = (acc << 5) | alphabet.index(c) }
    (0..15).map { |i| (acc >> (8 * (15 - i))) & 0xff }
  end
end
