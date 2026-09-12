# Stub slot for `lower::mocha` over spinel's `securerandom` package —
# `SecureRandom.stubs(:alphanumeric).returns(v)` becomes
# `SecureRandom.stub_alphanumeric(v)`, and the app's own
# `SecureRandom.alphanumeric(12)` (User::Bot's key, the join code)
# answers `v` until the helper's setup clears it.
#
# A REOPEN, and the last definition wins on spinel (verified: the
# package's `alphanumeric` is replaced, `random_bytes` beside it is
# not). There is no `alias_method` to reach the package's rendering
# from here, so the unstubbed arms re-render from `random_bytes`
# exactly as packages/securerandom/securerandom.rb does — rejection
# sampling below 248 for `alphanumeric`, the RFC 9562 v4 bits for
# `uuid`. Same bytes, same distribution; only the file differs.
#
# The ruby family gets a different file under this name
# (`project::SECURE_RANDOM_STUB_REOPEN`): there `alias_method` reaches
# the stdlib's own, so nothing is re-rendered.
#
# `Random.stubs(:uuid)` lands here too: `lower::random_formatter`
# grounds the app's `Random.uuid` to `SecureRandom.uuid`, so this is
# the slot that call reaches.
require "securerandom"

module SecureRandom
  STUB_ALPHANUMERIC_ON = [ false ]
  STUB_ALPHANUMERIC = [ "" ]
  STUB_UUID_ON = [ false ]
  STUB_UUID = [ "" ]

  def self.stub_alphanumeric(value)
    STUB_ALPHANUMERIC_ON[0] = true
    STUB_ALPHANUMERIC[0] = value
    nil
  end

  def self.stub_uuid(value)
    STUB_UUID_ON[0] = true
    STUB_UUID[0] = value
    nil
  end

  def self.clear_secure_random_stubs
    STUB_ALPHANUMERIC_ON[0] = false
    STUB_ALPHANUMERIC[0] = ""
    STUB_UUID_ON[0] = false
    STUB_UUID[0] = ""
    nil
  end

  def self.alphanumeric(n = 16)
    return STUB_ALPHANUMERIC[0] if STUB_ALPHANUMERIC_ON[0]
    out = +""
    while out.length < n
      want = n - out.length
      bytes = SecureRandom.random_bytes(want * 2 + 8)
      i = 0
      while i < bytes.bytesize && out.length < n
        b = bytes.getbyte(i)
        out << ALPHANUMERIC[b % 62] if b < 248
        i += 1
      end
    end
    out
  end

  def self.uuid
    return STUB_UUID[0] if STUB_UUID_ON[0]
    b = SecureRandom.random_bytes(16)
    h = +""
    i = 0
    while i < 16
      v = b.getbyte(i)
      v = (v & 0x0f) | 0x40 if i == 6
      v = (v & 0x3f) | 0x80 if i == 8
      h << HEXDIGITS[(v >> 4) & 0xf]
      h << HEXDIGITS[v & 0xf]
      i += 1
    end
    "#{h[0, 8]}-#{h[8, 4]}-#{h[12, 4]}-#{h[16, 4]}-#{h[20, 12]}"
  end
end
