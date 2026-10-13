# frozen_string_literal: true

# All expectations are literal, independently derived; each lane is a fresh process.
require "bundler/setup"
require "json"
lane, source = ARGV
external_cases = {
  "fasp_contract" => ["Fasp::Capability", %i[id id= version version= enabled enabled=]],
  "safe_delegate" => ["WebPushRequest", %i[standard endpoint key_auth key_p256dh]],
  "prefixed_delegate" => ["AccountSuggestions::Suggestion", %i[account account= account_id sources sources=]],
  "namespace_attributes" => ["TranslationService::Translation", %i[text text= provider provider= detected_source_language detected_source_language=]],
  "media_delegate" => ["StatusEdit::PreservedMediaAttachment", %i[id local? media_attachment media_attachment= description description=]],
  "policy_generated" => ["InteractionPolicy::SubPolicy", %i[public? followers? following? disabled? missing?]],
  "policy_plain" => ["InteractionPolicy::SubPolicy", %i[missing?]],
  "keyword_message" => ["Admin::SystemCheck::Message", %i[key value action critical]]
}
if source == "original"
  require File.join(ENV.fetch("MASTODON_APP"), "config/environment")
else
  if external_cases.key?(lane)
    # Host Rails supplies ONLY the real receiver objects/gems. Remove the
    # original exported class so missing output cannot fall back to its methods.
    require File.join(ENV.fetch("MASTODON_APP"), "config/environment")
    name, roots = external_cases.fetch(lane)
    parts = name.split("::")
    parent = parts[0...-1].inject(Object) { |owner, part| owner.const_get(part, false) }
    parent.send(:remove_const, parts.last)
  end
  load source
  if external_cases.key?(lane)
    owner = name.split("::").inject(Object) { |scope, part| scope.const_get(part, false) }
    roots.each do |root|
      location = owner.instance_method(root).source_location&.first
      raise "original/gem method leaked into output: #{name}##{root}" unless location&.start_with?(File.dirname(File.expand_path(source)) + "/")
    end
  end
end

checks = 0
observations = []
equal = ->(actual, expected) do
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
  checks += 1
  observations << actual
end
error_class = ->(&block) do
  block.call
rescue StandardError => error
  error.class.name
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
when "fasp_contract"
  a = Fasp::Capability.new(id: 17, version: "v7", enabled: "false")
  b = Fasp::Capability.new(id: 41, version: "v9", enabled: "1")
  equal.call(a.id, "17")
  equal.call(b.id, "41")
  equal.call(a.version, "v7")
  equal.call(b.version, "v9")
  equal.call(a.enabled, false)
  equal.call(b.enabled, true)
  equal.call(Fasp::Capability.new.enabled, false)
  a.enabled = nil
  equal.call(a.enabled, nil)
  equal.call(b.enabled, true)
  b.enabled = ""
  equal.call(b.enabled, nil)
  a.enabled = "OFF"
  equal.call(a.enabled, false)
  b.enabled = "yes"
  equal.call(b.enabled, true)
  a.enabled = 0
  equal.call(a.enabled, false)
  a.enabled = 1
  equal.call(a.enabled, true)
  b.id = nil
  equal.call(b.id, nil)
  equal.call(a.id, "17")
when "safe_delegate"
  # These are real unsaved AR objects, not replacements for their attribute store.
  a = Web::PushSubscription.new(standard: "false", endpoint: "https://north.invalid/a", key_auth: "north", key_p256dh: "λ-key")
  b = Web::PushSubscription.new(standard: "1", endpoint: "https://south.invalid/b", key_auth: "south", key_p256dh: "second-key")
  x = WebPushRequest.new(a)
  y = WebPushRequest.new(b)
  equal.call(x.standard, false)
  equal.call(y.standard, true)
  equal.call(x.legacy, true)
  equal.call(y.legacy, false)
  equal.call(x.endpoint, "https://north.invalid/a")
  equal.call(y.endpoint, "https://south.invalid/b")
  equal.call(x.key_auth, "north")
  equal.call(y.key_auth, "south")
  equal.call(x.key_p256dh, "λ-key")
  equal.call(y.key_p256dh, "second-key")
  a.standard = nil
  equal.call(x.standard, nil)
  equal.call(y.standard, true)
  b.standard = ""
  equal.call(y.standard, nil)
  a.standard = "OFF"
  equal.call(x.standard, false)
  b.standard = "yes"
  equal.call(y.standard, true)
  a.endpoint = "https://north.invalid/changed"
  equal.call(x.endpoint, "https://north.invalid/changed")
  equal.call(y.endpoint, "https://south.invalid/b")
  equal.call(error_class.call { WebPushRequest.new(nil).standard }, "ActiveSupport::DelegationError")
  equal.call(error_class.call { WebPushRequest.new(false).standard }, "NoMethodError")
  equal.call(error_class.call { x.standard(:unexpected) }, "ArgumentError")
  equal.call(error_class.call { x.standard(unexpected: true) }, "ArgumentError")
when "prefixed_delegate"
  a = Account.new(id: 17, username: "north")
  b = Account.new(id: 41, username: "south")
  x = AccountSuggestions::Suggestion.new(account: a, sources: ["north"])
  y = AccountSuggestions::Suggestion.new(account: b, sources: ["south"])
  equal.call(x.account_id, 17)
  equal.call(y.account_id, 41)
  equal.call(x.account.equal?(a), true)
  equal.call(y.account.equal?(b), true)
  equal.call(x.sources, ["north"])
  equal.call(y.sources, ["south"])
  x.sources = false
  equal.call(x.sources, false)
  equal.call(y.sources, ["south"])
  x.sources = nil
  equal.call(x.sources, nil)
  a.id = nil
  equal.call(x.account_id, nil)
  equal.call(y.account_id, 41)
  y.account = nil
  equal.call(error_class.call { y.account_id }, "ActiveSupport::DelegationError")
  x.account = false
  equal.call(error_class.call { x.account_id }, "NoMethodError")
  x.account = b
  equal.call(x.account_id, 41)
when "namespace_attributes"
  x = TranslationService::Translation.new(text: "nørth", detected_source_language: nil, provider: false)
  y = TranslationService::Translation.new(text: "south", detected_source_language: "de", provider: "original")
  equal.call(x.text, "nørth")
  equal.call(y.text, "south")
  equal.call(x.provider, false)
  equal.call(x.detected_source_language, nil)
  equal.call(y.detected_source_language, "de")
  x.text = nil
  equal.call(x.text, nil)
  equal.call(y.text, "south")
  x.provider = nil
  equal.call(x.provider, nil)
  equal.call(y.provider, "original")
  x.detected_source_language = "et"
  equal.call(x.detected_source_language, "et")
  equal.call(y.detected_source_language, "de")
  equal.call(TranslationService::Translation.new(nil).text, nil)
when "media_delegate"
  a = MediaAttachment.new(id: 17, remote_url: nil)
  b = MediaAttachment.new(id: 41, remote_url: "https://remote.invalid/media/41")
  x = StatusEdit::PreservedMediaAttachment.new(media_attachment: a, description: false)
  y = StatusEdit::PreservedMediaAttachment.new(media_attachment: b, description: nil)
  equal.call(x.id, 17)
  equal.call(y.id, 41)
  equal.call(x.local?, true)
  equal.call(y.local?, false)
  equal.call(x.description, false)
  equal.call(y.description, nil)
  a.id = nil
  equal.call(x.id, nil)
  equal.call(y.id, 41)
  x.description = "changed"
  equal.call(x.description, "changed")
  equal.call(y.description, nil)
  y.media_attachment = nil
  equal.call(error_class.call { y.local? }, "ActiveSupport::DelegationError")
  x.media_attachment = false
  equal.call(error_class.call { x.local? }, "NoMethodError")
when "policy_generated"
  # Literal expectations, not values read from the app's POLICY_FLAGS mapping.
  [
    [0, [false, false, false, false, true]],
    [2, [true, false, false, false, false]],
    [4, [false, true, false, false, false]],
    [8, [false, false, true, false, false]],
    [16, [false, false, false, true, false]],
    [6, [true, true, false, false, false]],
    [18, [true, false, false, true, false]]
  ].each do |bitmap, expected|
    object = InteractionPolicy::SubPolicy.new(bitmap)
    equal.call([object.public?, object.followers?, object.following?, object.disabled?, object.missing?], expected)
  end
  equal.call(InteractionPolicy::SubPolicy.new(2).public?, true)
  equal.call(InteractionPolicy::SubPolicy.new(4).public?, false)
when "policy_plain"
  a = InteractionPolicy::SubPolicy.new(0)
  b = InteractionPolicy::SubPolicy.new(4)
  equal.call(a.missing?, true)
  equal.call(b.missing?, false)
  equal.call(a.missing?, true)
when "keyword_message"
  # The app's real software/privacy checks call this constructor with critical:.
  a = Admin::SystemCheck::Message.new(:ordinary)
  b = Admin::SystemCheck::Message.new(:critical, "south", "/admin/updates", critical: true)
  c = Admin::SystemCheck::Message.new(:noncritical, nil, "/admin/rules", critical: false)
  equal.call(a.key, :ordinary)
  equal.call(a.value, nil)
  equal.call(a.action, nil)
  equal.call(a.critical, false)
  equal.call(b.critical, true)
  equal.call(b.key, :critical)
  equal.call(b.value, "south")
  equal.call(b.action, "/admin/updates")
  equal.call(c.critical, false)
  equal.call(c.value, nil)
  equal.call(c.action, "/admin/rules")
  equal.call(a.critical, false)
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
