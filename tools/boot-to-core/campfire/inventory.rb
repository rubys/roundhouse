# frozen_string_literal: true

# Source-site inventory, NOT a runtime coverage or compiler-support detector.
# Usage: bundle exec ruby inventory.rb APP_PATH ROUNDHOUSE_PATH
require "prism"
require "json"
require "digest"

app, compiler = ARGV.map { |path| File.realpath(path) }
families = {
  "association" => %w[has_many has_one belongs_to],
  "scope" => %w[scope],
  "enum" => %w[enum],
  "validation" => %w[validates validate validates_with],
  "model_callback" => %w[before_validation after_validation before_save after_save before_create after_create before_update after_update before_destroy after_destroy around_destroy after_commit after_create_commit after_update_commit after_destroy_commit],
  "secure_password_token" => %w[has_secure_password has_secure_token generates_token_for],
  "attributes_delegation" => %w[attr_reader attr_writer attr_accessor delegate class_attribute attribute],
  "structured_json" => %w[has_json has_delegated_json serialize store store_accessor],
  "storage_rich_text" => %w[has_one_attached has_many_attached has_rich_text variant],
  "controller_filter_auth" => %w[before_action after_action around_action prepend_before_action skip_before_action allow_unauthenticated_access require_unauthenticated_access allow_bot_access protect_from_forgery helper_method],
  "controller_browser_rate" => %w[allow_browser rate_limit],
  "cable_jobs" => %w[on_subscribe on_unsubscribe after_subscribe after_unsubscribe periodically stream_from stream_for queue_as retry_on discard_on],
  "routes" => %w[root resource resources namespace scope get post put patch delete direct],
  "schema" => %w[create_table string integer bigint text boolean datetime decimal float json binary references timestamps],
  "ruby_generation" => %w[define_method alias_method class_eval module_eval]
}
owners = {
  "association" => %w[src/ingest/model.rs src/lower/model_to_library/associations.rs runtime/ruby/active_record/base.rb],
  "scope" => %w[src/ingest/model.rs src/lower/model_to_library/mod.rs runtime/ruby/active_record/relation.rb],
  "enum" => %w[src/ingest/model.rs src/lower/model_to_library/schema.rs runtime/ruby/active_record/base.rb],
  "validation" => %w[src/ingest/model.rs src/lower/model_to_library/validations.rs runtime/ruby/active_record/base.rb],
  "model_callback" => %w[src/ingest/model.rs src/lower/model_to_library/markers.rs runtime/ruby/active_record/base.rb],
  "secure_password_token" => %w[src/lower/secure_password.rs src/lower/secure_token.rs src/lower/generates_token_for.rs runtime/ruby/bcrypt_facade.rb runtime/ruby/active_record/token_for.rb],
  "attributes_delegation" => %w[src/ingest/delegate.rs src/ingest/current_attributes.rs src/ingest/class_attribute.rs src/lower/model_to_library/schema.rs src/lower/model_to_library/markers.rs],
  "structured_json" => %w[src/lower/has_json.rs runtime/spinel/schematized_json.rb runtime/spinel/scaffold/ruby_overlay/runtime/schematized_json.rb],
  "storage_rich_text" => %w[src/lower/attached.rs src/lower/rich_text.rs runtime/ruby/active_storage.rb runtime/ruby/action_text.rb],
  "controller_filter_auth" => %w[src/ingest/controller.rs src/lower/controller_to_library/mod.rs runtime/ruby/action_controller/base.rb],
  "controller_browser_rate" => %w[src/ingest/allow_browser.rs src/ingest/rate_limit.rs runtime/ruby/action_controller/browser_blocker.rb runtime/ruby/action_controller/rate_limiter.rb],
  "cable_jobs" => %w[src/ingest/channel_callbacks.rs runtime/spinel/action_cable.rb runtime/spinel/scaffold/ruby_overlay/runtime/action_cable.rb runtime/ruby/active_job.rb],
  "routes" => %w[src/ingest/routes.rs src/lower/routes.rs src/lower/routes_to_library/mod.rs runtime/ruby/action_dispatch/router.rb],
  "schema" => %w[src/ingest/schema.rs src/analyze/mod.rs src/lower/model_to_library/schema.rs],
  "ruby_generation" => %w[tools/boot-to-core/capture.rb tools/native-observer/observer.c]
}
sites = families.transform_values { [] }
source_hashes = {}
paths = Dir[File.join(app, "{app,lib,config}/**/*.rb")] + [File.join(app, "db/schema.rb")]
paths.sort.each do |path|
  source = File.read(path)
  source_hashes[path.delete_prefix(app + "/")] = Digest::SHA256.hexdigest(source)
  parsed = Prism.parse(source)
  raise "cannot parse #{path}: #{parsed.errors.map(&:message).join('; ')}" unless parsed.success?
  walk = ->(node, context) do
    return unless node
    context += [node.constant_path.location.slice] if node.is_a?(Prism::ClassNode) || node.is_a?(Prism::ModuleNode)
    context += ["##{node.name}"] if node.is_a?(Prism::DefNode)
    if node.is_a?(Prism::CallNode)
      relative = path.delete_prefix(app + "/")
      families.each do |family, names|
        next unless names.include?(node.name.to_s)
        next if family == "routes" && relative != "config/routes.rb"
        next if family == "schema" && relative != "db/schema.rb"
        next if relative == "config/routes.rb" && family != "routes"
        next if relative == "db/schema.rb" && family != "schema"
        next if node.receiver && !node.receiver.is_a?(Prism::SelfNode) && !%w[schema storage_rich_text ruby_generation].include?(family)
        sites.fetch(family) << { path: relative, line: node.location.start_line,
          column: node.location.start_column, name: node.name, context: context,
          declaration: node.location.slice.lines.first.strip }
      end
    end
    node.compact_child_nodes.each { |child| walk.call(child, context) }
  end
  walk.call(parsed.value, [])
end
owner_hashes = owners.values.flatten.uniq.to_h do |path|
  [path, Digest::SHA256.file(File.join(compiler, path)).hexdigest]
end
puts JSON.pretty_generate(scope: "Campfire syntax candidates only; site counts are not generated-method or supported-family counts",
  app_source_sha256: source_hashes, compiler_owner_sha256: owner_hashes,
  families: sites.to_h { |family, entries| [family, { count: entries.length, owners: owners.fetch(family), sites: entries }] },
  savings: { demonstrated_dsl_families_replaced: 0, compiler_or_runtime_lines_removed: 0,
    caveat: "Capturing schema/enum bodies does not replace inherited runtime, mutable state, callback scheduling, route metadata or persistence. Three verified source leaves are not DSL-family replacement evidence." })
