# frozen_string_literal: true

# Source-site inventory, not an executed generator count or support ledger.
require "prism"
require "json"
root = File.expand_path(ARGV.fetch(0))
families = {
  "attribute" => %w[attribute attr_reader attr_writer attr_accessor cattr_accessor mattr_accessor class_attribute],
  "enum" => %w[enum], "delegate" => %w[delegate delegated_type],
  "association" => %w[belongs_to has_one has_many has_and_belongs_to_many],
  "scope" => %w[scope], "markdown" => %w[has_markdown], "position" => %w[positioned_within],
  "generation" => %w[define_method class_eval module_eval],
  "storage" => %w[has_one_attached has_many_attached],
  "security" => %w[has_secure_password has_secure_token], "serialization" => %w[serialize],
  "callback" => %w[before_create before_save before_action after_action around_create after_save_commit after_create_commit after_update_commit after_destroy_commit after_create after_update after_destroy],
  "framework_hook" => %w[included class_methods on_load to_prepare after_initialize]
}
lookup = families.flat_map { |family, names| names.map { |name| [name.to_sym, family] } }.to_h
sites = []
walk = ->(node, file) do
  if node.is_a?(Prism::CallNode) && (family = lookup[node.name])
    sites << { family: family, method: node.name, file: file.delete_prefix(root + "/"),
      line: node.location.start_line, column: node.location.start_column + 1,
      arguments: node.arguments&.location&.slice }
  end
  node.compact_child_nodes.each { |child| walk.call(child, file) }
end
Dir[File.join(root, "{app,lib,config}/**/*.rb")].sort.each do |file|
  parsed = Prism.parse_file(file)
  raise "parse failure #{file}: #{parsed.errors.map(&:message)}" unless parsed.success?
  walk.call(parsed.value, file)
end
puts JSON.pretty_generate(scope: "source calls in app/lib/config Ruby; no ERB, route expansion or generator execution count",
  counts: sites.group_by { |site| site[:family] }.transform_values(&:length), sites: sites)
