# frozen_string_literal: true

# Proposed public contracts for the next exact generic snapshot. The original
# runs use real pinned classes, never replacement targets. Standalone runs may
# use only classes present in the Core/emitted loader: no Rails requires here.
require "json"
require "digest"
original = ARGV.fetch(0) == "original"
if original
  require File.join(ENV.fetch("CORE_RUBY_APP"), "config/environment")
  Rails.application.eager_load!
else
  load ARGV.fetch(0)
  raise "Rails leaked into standalone contract" if defined?(Rails) || defined?(ActiveRecord)
end

observations = {}
assert = ->(label, actual, expected) do
  raise "#{label}: expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
  observations[label] = actual
end
raises = ->(label, expected, &operation) do
  error = begin
    operation.call
    nil
  rescue StandardError => exception
    exception
  end
  assert.call(label, error&.class&.name, expected)
  error
end

lane = ARGV.fetch(1)
methods = case lane
when "namespace_accessor"
  first = +"https://alpha.example.test:8443/a"
  location = Opengraph::Location.new(first)
  assert.call("constructor", location.url, "https://alpha.example.test:8443/a")
  assert.call("constructor identity", location.url.equal?(first), true)
  assert.call("nil constructor", Opengraph::Location.new(nil).url, nil)
  second = +"http://beta.example.test:81/b?key=29"
  result = location.public_send(:url=, second)
  assert.call("setter return identity", result.equal?(second), true)
  assert.call("different setter value", location.url, "http://beta.example.test:81/b?key=29")
  assert.call("setter storage identity", location.url.equal?(second), true)
  assert.call("nil setter return", location.public_send(:url=, nil), nil)
  assert.call("nil setter storage", location.url, nil)
  assert.call("other instance unchanged", Opengraph::Location.new(first).url, "https://alpha.example.test:8443/a")
  assert.call("getter public", location.respond_to?(:url), true)
  # The app overrides this reader privately; its parser is outside this cut.
  assert.call("parsed_url is not made public", location.respond_to?(:parsed_url), false)
  assert.call("parsed_url writer stays public", location.respond_to?(:parsed_url=), true)
  shadow = +"separate public writer value"
  assert.call("parsed_url writer return identity", location.public_send(:parsed_url=, shadow).equal?(shadow), true)
  assert.call("parsed_url writer does not overwrite url", location.url, nil)
  assert.call("parsed_url nil writer return", location.public_send(:parsed_url=, nil), nil)
  assert.call("parsed_url nil writer leaves url alone", location.url, nil)
  { Opengraph::Location => %i[initialize url url= parsed_url parsed_url=] }
when "platform_inherited"
  inputs = ["iPhone", "Android", "Macintosh", nil, "iPad", "Android iPhone"]
  expected = [[true, false, true, false], [false, true, true, false],
    [false, false, false, true], [false, false, false, true],
    [true, false, true, false], [true, true, true, false]]
  inputs.zip(expected).each_with_index do |(agent, values), index|
    platform = ApplicationPlatform.new(agent)
    %i[ios? android? mobile? desktop?].zip(values).each do |name, value|
      assert.call("#{index} #{name}", platform.public_send(name), value)
    end
  end
  assert.call("inherited match? stays private", ApplicationPlatform.new(nil).respond_to?(:match?), false)
  { ApplicationPlatform => %i[initialize ios? android? mobile? desktop? match? user_agent_string user_agent_string=] }
when "delegate_current"
  current = Current.new
  current.request = nil
  assert.call("nil prefixed host", current.request_host, nil)
  assert.call("nil prefixed protocol", current.request_protocol, nil)
  block_ran = false
  assert.call("nil ignores forwarded arguments", current.request_host(:unused) { block_ran = true }, nil)
  assert.call("nil does not run block", block_ran, false)
  first = ActionDispatch::Request.new("HTTP_HOST" => "alpha.example.test:8443", "rack.url_scheme" => "https")
  assert.call("real request setter identity", current.public_send(:request=, first).equal?(first), true)
  assert.call("non-nil host", current.request_host, "alpha.example.test")
  assert.call("non-nil protocol", current.request_protocol, "https://")
  raises.call("non-nil forwards invalid arity", "ArgumentError") { current.request_host(:unused) }
  second = ActionDispatch::Request.new("SERVER_NAME" => "beta.example.test", "SERVER_PORT" => "81", "rack.url_scheme" => "http")
  current.request = second
  assert.call("different request host", current.request_host, "beta.example.test")
  assert.call("different request protocol", current.request_protocol, "http://")
  current.request = nil
  assert.call("nil again has no stale host", current.request_host, nil)
  assert.call("prefix does not add host", current.respond_to?(:host), false)
  { Current => %i[initialize request request= request_host request_protocol],
    ActionDispatch::Request => %i[initialize host protocol] }
when "delegate_filter"
  content = ActionText::Content.new("<p><strong>AL</strong> alpha</p>", canonicalize: false)
  filter = ActionText::Content::Filter.new(content)
  assert.call("generated fragment is public", filter.respond_to?(:fragment), true)
  assert.call("content reader stays private", filter.respond_to?(:content), false)
  assert.call("actual fragment identity", filter.fragment.equal?(content.fragment), true)
  assert.call("actual fragment text", filter.fragment.find_all("p").first.text, "AL alpha")
  raises.call("target arity retained", "ArgumentError") { filter.fragment(:unexpected) }
  raises.call("nil target delegation error", "ActiveSupport::DelegationError") { ActionText::Content::Filter.new(nil).fragment }
  error = raises.call("non-nil missing target method is not delegation error", "NoMethodError") do
    ActionText::Content::Filter.new("not a content object").fragment
  end
  assert.call("missing target method name", error.name.to_s, "fragment")
  { ActionText::Content::Filter => %i[initialize fragment content], ActionText::Content => %i[initialize fragment] }
when "delegate_presentation"
  context = ActionView::Base.empty
  # The message is unused by these real generated delegates; render is NOT a root.
  presentation = Messages::AttachmentPresentation.new(nil, context: context)
  assert.call("generated link_to is public", presentation.respond_to?(:link_to), true)
  assert.call("context reader stays private", presentation.respond_to?(:context), false)
  assert.call("positional and keyword forwarding", presentation.link_to("Ada →", "/rooms/7", class: "primary").to_s,
    '<a class="primary" href="/rooms/7">Ada →</a>')
  assert.call("different arguments", presentation.link_to("Grace", "/rooms/29", title: "Compiler").to_s,
    '<a title="Compiler" href="/rooms/29">Grace</a>')
  block_calls = 0
  html = presentation.link_to("/rooms/41", class: "block") { block_calls += 1; "Open AL" }
  assert.call("block and keyword forwarding", html.to_s, '<a class="block" href="/rooms/41">Open AL</a>')
  assert.call("forwarded block runs once", block_calls, 1)
  raises.call("nil context delegation error", "ActiveSupport::DelegationError") do
    Messages::AttachmentPresentation.new(nil, context: nil).link_to("Ada", "/rooms/7")
  end
  { Messages::AttachmentPresentation => %i[initialize link_to context], ActionView::Base => %i[initialize link_to] }
else
  raise "unknown candidate #{lane}"
end

result = { lane: lane, checks: observations.length, observations: observations }
if original
  result[:methods] = methods.flat_map do |owner, names|
    names.map do |name|
      method = owner.instance_method(name)
      path, line = method.source_location
      visibility = owner.public_method_defined?(name) ? "public" : owner.protected_method_defined?(name) ? "protected" : "private"
      { receiver: owner.name, method: name, owner: method.owner.name, parameters: method.parameters,
        visibility: visibility, source: [path, line], source_sha256: path && File.file?(path) && Digest::SHA256.file(path).hexdigest }
    end
  end
end
puts JSON.generate(result)
