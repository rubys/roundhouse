# Executed inside the pinned Campfire application by
# `scripts/campfire-route-inventory` after Rails has loaded its actual route set.

require "digest"
require "json"

root = Rails.root
route_file = root.join("config/routes.rb")
lockfile = root.join("Gemfile.lock")

scope_for = lambda do |controller, path|
  if path == "/cable"
    "action_cable"
  elsif controller&.start_with?("active_storage/")
    "active_storage"
  elsif controller&.start_with?("action_mailbox/")
    "action_mailbox"
  elsif controller&.start_with?("rails/conductor/")
    "rails_conductor"
  elsif controller&.start_with?("turbo/native/")
    "turbo_native"
  elsif controller == "rails/health"
    "rails_health"
  else
    "campfire"
  end
end

routes = Rails.application.routes.routes.each_with_index.map do |route, index|
  path = route.path.spec.to_s
  controller = route.defaults[:controller]&.to_s
  action = route.defaults[:action]&.to_s
  {
    "order" => index + 1,
    "name" => route.name&.to_s,
    "verb" => route.verb.to_s,
    "path" => path,
    "controller" => controller,
    "action" => action,
    "scope" => scope_for.call(controller, path)
  }
end

rails_revision = lockfile.read[/^  remote: https:\/\/github\.com\/rails\/rails\.git\n  revision: ([0-9a-f]+)$/m, 1]
payload = {
  "schema_version" => 1,
  "source" => {
    "campfire_sha" => ENV.fetch("CAMPFIRE_SOURCE_SHA"),
    "rails_version" => Rails.version,
    "rails_revision" => rails_revision,
    "routes_sha256" => Digest::SHA256.file(route_file).hexdigest,
    "gemfile_lock_sha256" => Digest::SHA256.file(lockfile).hexdigest
  },
  "counts" => routes.group_by { |route| route.fetch("scope") }.transform_values(&:length),
  "routes" => routes
}

puts JSON.pretty_generate(payload)
