# Driver for tests/spinel_request_body_cap.rs — the request-body cap in
# the spinel lane's HTTP server (`runtime/spinel/tep/`).
#
# Run under plain CRuby against the real servers over a scripted socket
# (tests/tep_server_harness.rb).
#
# Headers were capped (MAX_REQUEST_BYTES, 64 KiB) but the body was not:
# `Request#content_length` was the header's bare `.to_i`, and every drain
# looped recv-and-append until that many bytes had arrived. A client could
# declare `Content-Length: 10737418240` and stream, and the worker held
# all of it in one String (rebuilt by `+` on every chunk) before the app
# ever saw the request. A body that large must be refused from the
# header — 413, before a single body byte is read — and a Content-Length
# that is not a plain decimal byte count is a 400, as Puma (the Rails
# lane's server) answers it, rather than whatever `.to_i` makes of it.
#
#     ruby tests/spinel_request_body_cap.rb            # default cap
#     TEP_MAX_BODY_BYTES=1024 ruby tests/spinel_request_body_cap.rb

require_relative "tep_server_harness"

# The attacker's request: a 10 GiB declaration and 8 MiB actually sent,
# which the unfixed drains read to EOF and handed to the app.
ATTACK = post(10 * 1024 * 1024 * 1024, "x" * (8 * 1024 * 1024))

cap_env = ENV["TEP_MAX_BODY_BYTES"].to_s

if cap_env.empty?
  SERVERS.each_key do |s|
    r = serve(s, ATTACK)
    status, recvs, bodies, raised = r
    # One recv is the header read itself (which may carry the first few
    # KiB of body with it); any further recv is the body drain.
    check(
      "#{s}: a 10 GiB Content-Length is refused 413 before the body is read",
      raised.nil? && status == 413 && recvs == 1 && bodies.empty?,
      describe(*r)
    )

    # Not a decimal byte count: 400, Puma's answer (it rejects any
    # Content-Length matching /[^\d]/). `.to_i` read "12abc" as 12 and
    # "-1" as a length that drained nothing.
    [["12abc", "trailing junk"], ["-1", "a negative length"], ["+5", "a sign"]].each do |value, what|
      r = serve(s, post(value, "title=hello"))
      status, _recvs, bodies, raised = r
      check(
        "#{s}: #{what} in Content-Length is a 400",
        raised.nil? && status == 400 && bodies.empty?,
        describe(*r)
      )
    end

    # Well-formed, just past int64. Too large, so 413 — and decided from
    # the digit count, never by converting: spinel's Integer is a fixed
    # int64, where the conversion itself is the hazard.
    r = serve(s, post("1" * 25, "title=hello"))
    status, recvs, bodies, raised = r
    check(
      "#{s}: a Content-Length past int64 is a 413, without converting it",
      raised.nil? && status == 413 && recvs == 1 && bodies.empty?,
      describe(*r)
    )

    # An explicitly empty Content-Length is invalid (RFC 9112, 6.3
    # rule 5); Puma 8.0.2 refuses it too. Absence still means no body.
    r = serve(s, post("", ""))
    status, _recvs, bodies, raised = r
    check(
      "#{s}: an empty Content-Length is a 400",
      raised.nil? && status == 400 && bodies.empty?,
      describe(*r)
    )

    r = serve(s, post(11, "title=hello"))
    status, _recvs, bodies, raised = r
    check(
      "#{s}: an ordinary form post still reaches the app intact",
      raised.nil? && status == 200 && bodies == ["title=hello"],
      describe(*r)
    )

    r = serve(s, post(nil, ""))
    status, _recvs, bodies, raised = r
    check(
      "#{s}: a request with no Content-Length still serves",
      raised.nil? && status == 200 && bodies == [""],
      describe(*r)
    )
  end

  check(
    "the default cap is 100 MiB",
    Tep.respond_to?(:max_body_bytes) && Tep.max_body_bytes == 100 * 1024 * 1024
  )

  # Leading zeros are not magnitude: Puma reads a zero-padded length as
  # its value (`.to_i`), so 22 characters of "...0011" is eleven bytes,
  # not a saturated "too large".
  check(
    "a zero-padded byte count reads as its value",
    Tep.decimal_byte_count("0" * 20 + "11") == 11,
    "got #{Tep.decimal_byte_count("0" * 20 + "11")}"
  )
else
  # The harness says what cap the override must produce. Over-18-digit
  # values used to saturate to the same 10^18 a huge Content-Length
  # saturates to, so the override became that ceiling and an over-18-digit
  # length compared EQUAL to it and passed: a zero-padded small value, or
  # any absurd one, switched the cap off.
  expect = Integer(ENV.fetch("EXPECT_CAP"))
  check(
    "TEP_MAX_BODY_BYTES=#{cap_env} makes the cap #{expect}",
    Tep.max_body_bytes == expect,
    "got #{Tep.max_body_bytes}"
  )

  SERVERS.each_key do |s|
    r = serve(s, post("9" * 25, "x"))
    status, recvs, bodies, raised = r
    check(
      "#{s}: under this override a 25-digit Content-Length is still a 413",
      raised.nil? && status == 413 && recvs == 1 && bodies.empty?,
      describe(*r)
    )

    # The boundary, where the cap is small enough to send.
    next if expect > 1 << 20

    r = serve(s, post(expect, "a" * expect))
    status, _recvs, bodies, raised = r
    check(
      "#{s}: a body exactly at TEP_MAX_BODY_BYTES is served",
      raised.nil? && status == 200 && bodies.map(&:bytesize) == [expect],
      describe(*r)
    )

    r = serve(s, post(expect + 1, "a" * (expect + 1)))
    status, recvs, bodies, raised = r
    check(
      "#{s}: one byte over TEP_MAX_BODY_BYTES is a 413",
      raised.nil? && status == 413 && recvs == 1 && bodies.empty?,
      describe(*r)
    )
  end
end

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
