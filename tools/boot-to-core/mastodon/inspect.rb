# frozen_string_literal: true

# Read-only method provenance from the real booted app. No DSL is replayed.
require "bundler/setup"
require "json"
require "digest"
compiler = File.expand_path(ARGV.fetch(0))
require File.join(compiler, "tools/boot-to-core/capture")
BootToCore.capture { require File.join(ENV.fetch("MASTODON_APP"), "config/environment") }
BootToCore.capture do
  LoginActivity.define_attribute_methods
  Fasp::Capability.define_attribute_methods
end
raise "native collector failed" unless NativeDefineMethodObserver.failures.empty?

selected = {
  Fasp::Capability => %i[initialize enabled enabled= attribute attribute=],
  LoginActivity => %i[otp? authentication_method_for_database public_send],
  StatusEdit => %i[local? status association],
  WebPushRequest => %i[initialize standard endpoint key_auth key_p256dh web_push_subscription legacy],
  AccountSuggestions::Suggestion => %i[initialize account account= account_id sources sources=],
  StatusEdit::PreservedMediaAttachment => %i[initialize media_attachment media_attachment= local? id description description=],
  TranslationService::Translation => %i[initialize text text= provider detected_source_language],
  InteractionPolicy::SubPolicy => %i[initialize public? followers? following? disabled? missing?],
  ActiveModel::Type::Boolean => %i[cast_value],
  ActiveModel::AttributeSet => %i[fetch_value write_from_user]
}
methods = selected.flat_map do |owner, names|
  names.map do |name|
    method = owner.instance_method(name)
    record = BootToCore.source_record(method)
    closure = %i[name value key].filter_map do |local|
      next unless record.binding&.local_variable_defined?(local)
      value = record.binding.local_variable_get(local)
      next unless [String, Symbol, Integer, NilClass, TrueClass, FalseClass].include?(value.class)
      [local, { type: value.class.name, value: value, frozen: value.frozen? }]
    end.to_h
    path = method.source_location&.first
    { requested_owner: owner.name, method: name, definition_owner: method.owner.name,
      source_location: method.source_location, source_sha256: path && File.file?(path) ? Digest::SHA256.file(path).hexdigest : nil,
      parameters: method.parameters, origin: record.origin, opaque_reason: record.reason,
      body: record.node&.location&.slice, closure: closure }
  end
end
puts JSON.pretty_generate(ruby: RUBY_DESCRIPTION, compiler: compiler,
  capture_sha256: Digest::SHA256.file(File.join(compiler, "tools/boot-to-core/capture.rb")).hexdigest,
  loaded_gems: Gem.loaded_specs.transform_values { |gem| gem.version.to_s }, methods: methods)
