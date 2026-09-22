# Minitest-shaped, CRuby-only, same quarantine as broadcasts_test.rb
# (`project.rs::spin_shape` keys on the `Minitest::Test` parent).
#
# `Turbo::Streams::StreamName` signs and verifies the name a
# `<turbo-cable-stream-source>` carries, the way turbo-rails does. The
# bytes below were MINTED BY RAILS, not by this runtime: campfire under
# Rails 8.2 / turbo-rails 2.0.23, `bin/rails runner -e test` with the
# app's own test secret —
#
#   Turbo::StreamsChannel.signed_stream_name(["rooms"])
#   Turbo::StreamsChannel.signed_stream_name(["gid://campfire/Room/1", :messages])
#
# so a match here is interoperation, not self-consistency: a name this
# runtime writes into a page verifies under Rails, and a name Rails
# signed verifies here. Rides into every ruby emit and is run by
# tests/ruby_toolchain.rs.
require "minitest/autorun"
require_relative "test_helper"
require_relative "../runtime/turbo_streams"

class TurboStreamsTest < Minitest::Test
  SECRET = "1b41e6405f13aeeec0ca65ba29cff9c344162f8c79abcfa3778455807f3780f1" \
           "7aced7729bd5abe8a18e7c2368989a3311ab00e413d764e9352bec7d79a2103c"
  ROOMS = "InJvb21zIg==--84da2a38caae81bc84c9070a3aec6366a3c2dd04d58e0b68d062d87afbb11846"
  ROOM_MESSAGES = "ImdpZDovL2NhbXBmaXJlL1Jvb20vMTptZXNzYWdlcyI=" \
                  "--78b04b3c90b634ae0d74b2f1e38d01c339eaa0b87bc6d05e8a200a932e0e0287"

  def setup
    @was = Rails.secret_key_base
    Rails.secret_key_base = SECRET
  end

  def teardown
    Rails.secret_key_base = @was
  end

  def test_signs_the_bytes_rails_mints
    assert_equal ROOMS, Turbo::Streams::StreamName.signed("rooms")
    assert_equal ROOM_MESSAGES, Turbo::Streams::StreamName.signed("gid://campfire/Room/1:messages")
  end

  def test_verifies_a_name_rails_signed
    assert_equal "rooms", Turbo::Streams::StreamName.verified(ROOMS)
    assert_equal "gid://campfire/Room/1:messages", Turbo::Streams::StreamName.verified(ROOM_MESSAGES)
  end

  def test_refuses_what_rails_refuses
    # The verified name handed back bare — campfire's "an unsigned stream
    # name is rejected" — and a digest for another secret, a tampered
    # payload, an empty string, nil.
    assert_nil Turbo::Streams::StreamName.verified("rooms")
    assert_nil Turbo::Streams::StreamName.verified(ROOMS.sub("84da", "84db"))
    assert_nil Turbo::Streams::StreamName.verified("InJvb21yIg==" + ROOMS[12..])
    assert_nil Turbo::Streams::StreamName.verified("")
    assert_nil Turbo::Streams::StreamName.verified(nil)
    Rails.secret_key_base = "another"
    assert_nil Turbo::Streams::StreamName.verified(ROOMS)
  end

  def test_the_verifier_facade_is_the_same_pair
    assert_equal ROOMS, Turbo.signed_stream_verifier.generate("rooms")
    assert_equal "rooms", Turbo.signed_stream_verifier.verified(ROOMS)
  end

  def test_turbo_stream_from_writes_the_signed_name
    html = ActionView::ViewHelpers.turbo_stream_from("rooms", "Turbo::StreamsChannel")
    assert_includes html, %(signed-stream-name="#{ROOMS}")
    assert_includes html, %(channel="Turbo::StreamsChannel")
  end
end
