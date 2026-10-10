# frozen_string_literal: true

# All expectations are literal, independently derived; each lane is a fresh process.
require "bundler/setup"
require "json"
lane, source = ARGV
if source == "original"
  require File.join(ENV.fetch("MASTODON_APP"), "config/environment")
else
  load source
end

checks = 0
observations = []
equal = ->(actual, expected) do
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
  checks += 1
  observations << actual
end

case lane
when "ordinary"
  # Addressable remains an explicit external input-object boundary in ALL lanes.
  require "addressable/uri"
  object = HttpSignatureDraft.new(nil, "unused-disposable-key-id")
  [
    ["get", "https://example.invalid/users/alice", "get /users/alice"],
    ["post", "https://example.invalid/inbox?limit=17&tag=Ruby", "post /inbox?limit=17&tag=Ruby"],
    ["delete", "https://example.invalid/api/statuses/29?", "delete /api/statuses/29?"],
    ["patch", "https://example.invalid/a%2Fb?x=7%2B9#ignored", "patch /a%2Fb?x=7%2B9"],
    ["GET", "https://example.invalid/CaseSensitive?q=NoNormalization", "GET /CaseSensitive?q=NoNormalization"]
  ].each do |verb, url, expected|
    equal.call(object.request_target(verb, Addressable::URI.parse(url)), expected)
  end
when "accessor"
  a = ResponseWithLimit.new("north λ", 17)
  b = ResponseWithLimit.new("south", -9)
  equal.call(a.response, "north λ")
  equal.call(a.limit, 17)
  equal.call(b.response, "south")
  equal.call(b.limit, -9)
  equal.call(a.response, "north λ")
when "constant"
  object = ASCIIFolding.new
  [
    ["ÀáÇçĐđÉéŴŵŸÿŹź", "AaCcDdEeWwYyZz"],
    ["Crème brûlée—Łódź", "Creme brulee—Lodz"],
    ["Tallinn λ 日本 😀", "Tallinn λ 日本 😀"],
    ["é/e\u0301", "e/e\u0301"],
    ["Þþ ß Ææ Œœ", "Þþ ß Ææ Œœ"],
    ["", ""],
    ["õÖüÄ", "oOuA"],
    ["111 _tag! #42", "111 _tag! #42"]
  ].each { |input, expected| equal.call(object.fold(input), expected) }
  input = "ŊŋÑñ".dup
  equal.call(object.fold(input), "NnNn")
  equal.call(input, "ŊŋÑñ")
when "generated_reference"
  otp = LoginActivity.new(authentication_method: "otp")
  password = LoginActivity.new(authentication_method: "password")
  equal.call(otp.otp?, true)
  equal.call(otp.password?, false)
  equal.call(password.otp?, false)
  equal.call(password.password?, true)
  equal.call(LoginActivity.new.otp?, false)
  capability = Fasp::Capability.new(id: "probe", version: "v7", enabled: "false")
  equal.call(capability.enabled, false)
  capability.enabled = "1"
  equal.call(capability.enabled, true)
  equal.call(Fasp::Capability.new.enabled, false)
  settings = UserSettings.new(always_send_emails: true, aggregate_reblogs: false)
  equal.call(settings.always_send_emails, true)
  equal.call(settings.aggregate_reblogs, false)
  equal.call(UserSettings.new({}).always_send_emails, false)
  user = User.new(external: "false")
  equal.call(user.external, false)
  user.external = "1"
  equal.call(user.external, true)
  local_status = Status.new(id: 17, account: Account.new(id: 29, username: "probe"))
  remote_status = Status.new(id: 41, uri: "https://remote.invalid/status/41", account: Account.new(id: 43, username: "remote", domain: "remote.invalid"))
  equal.call(StatusEdit.new(status: local_status).local?, true)
  equal.call(StatusEdit.new(status: remote_status).local?, false)
else
  raise "unknown contract #{lane}"
end
puts "MASTODON_CONTRACT=" + JSON.generate(checks: checks, observations: observations)
