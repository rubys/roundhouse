# frozen_string_literal: true

require "json"
if ARGV.fetch(0) == "original"
  require File.join(ENV.fetch("CORE_RUBY_APP"), "config/environment")
  Rails.application.eager_load!
else
  load ARGV.fetch(0)
  raise "Rails leaked into standalone contract" if defined?(Rails) || defined?(ActiveRecord)
end

# Independent literal expectations; the read-only cases use asymmetric
# constructor inputs. The original leaf case remains unchanged.
lane = ARGV.fetch(1, "leaf")
case lane
when "leaf"
  room = Room.new
  user = User.new
  observations = [room.default_involvement, user.to_attachable_partial_path,
                  user.to_editor_content_attachment_partial_path]
  expected = ["mentions", "users/mention", "users/mention"]
when "mention"
  observations = [User.new.attachable_content_type]
  expected = ["application/vnd.campfire.mention"]
when "platform"
  observations = ["iPhone", "Android", "Macintosh", nil].flat_map do |agent|
    object = ApplicationPlatform.new(agent)
    [object.ios?, object.android?, object.mobile?, object.desktop?]
  end
  expected = [true, false, true, false, false, true, true, false,
              false, false, false, true, false, false, false, true]
when "sound"
  observations = [Sound.new(name: "bell", text: "🔔"),
    Sound.new(name: "56k", image: { name: "56k.webp", width: 79, height: 33 })].flat_map do |sound|
    [sound.name, sound.asset_path, sound.text, sound.image&.asset_path]
  end
  expected = ["bell", "bell.mp3", "🔔", nil, "56k", "56k.mp3", nil, "sounds/56k.webp"]
else
  raise "no behavioral contract for #{lane}"
end
raise "contract differs: #{observations.inspect}" unless observations == expected
puts JSON.generate(checks: expected.length, observations: observations)
