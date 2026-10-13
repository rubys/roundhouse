# frozen_string_literal: true

require "json"
require File.join(ENV.fetch("CORE_RUBY_APP"), "config/environment")
Rails.application.eager_load!
require "rack/mock"
checks = 0
assert = ->(actual, expected) do
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
  checks += 1
end
response = Rack::MockRequest.new(Rails.application).get("/up")
assert.call(response.status, 200)
raise "health response lacks green marker" unless response.body.include?("background-color: green")
checks += 1

# Unsaved real ActiveRecord objects, exercising asymmetric values without
# mail, callbacks, jobs, network requests, or writes to application tables.
[["Ada Lovelace", "computing", "AL", "Ada Lovelace – computing"],
 ["Grace Hopper", "", "GH", "Grace Hopper"]].each do |name, bio, initials, title|
  user = User.new(name: name, bio: bio)
  assert.call(user.initials, initials)
  assert.call(user.title, title)
  assert.call(user.active?, true)
  user.status = :deactivated
  assert.call(user.active?, false)
  assert.call(user.deactivated?, true)
end
assert.call(Room.new.default_involvement, "mentions")
assert.call(User.new.to_attachable_partial_path, "users/mention")
assert.call(User.new.to_editor_content_attachment_partial_path, "users/mention")
assert.call(User.new.attachable_content_type, "application/vnd.campfire.mention")
puts JSON.generate(checks: checks, health_status: response.status, rails: Rails.version,
  ruby: RUBY_DESCRIPTION, gems: Gem.loaded_specs.transform_values { |gem| gem.version.to_s },
  attribute_methods_generated: User.send(:attribute_methods_generated?))
