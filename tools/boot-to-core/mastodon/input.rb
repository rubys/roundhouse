# frozen_string_literal: true

# The real app and bundle are supplied by the runner. No app source is rewritten.
app = ENV.fetch("MASTODON_APP")
lane = ENV.fetch("MASTODON_LANE")
require File.join(app, "config/boot")

booted = false
at_exit do
  opaque = BootToCore.records.values.select { |record| record.respond_to?(:reason) && record.reason }
  failures = defined?(NativeDefineMethodObserver) ? NativeDefineMethodObserver.failures : []
  warn "MASTODON_OBSERVATION=" + JSON.generate(booted: booted,
    opaque_definitions: opaque.map { |record| { owner: record.owner.name, method: record.name,
      origin: record.origin, reason: record.reason, source_location: record.definition.source_location } },
    opaque_events: BootToCore.events.select { |event| event[:kind].start_with?("opaque") },
    observer_failures: failures.map { |owner, name, error| { owner: owner.name, method: name,
      kind: error.class.name, message: error.message, stack: error.backtrace&.first(8) } },
    scalar_constants: BootToCore.events.select { |event| event[:kind] == "scalar_constant" },
    immutable_constants: BootToCore.events.select { |event| event[:kind] == "immutable_constant" })
end

# Observe the failing exporter input without changing admission or retrying it.
BootToCore.singleton_class.prepend(Module.new do
  def record_eval(owner, source, path, first_line)
    super
  rescue BootToCore::Unsupported
    warn "MASTODON_EVAL_BOUNDARY=" + JSON.generate(owner: owner.name, path: path, line: first_line,
      sha256: Digest::SHA256.hexdigest(source), source: source, stack: caller.first(8))
    raise
  end

  def source_record(method)
    super
  rescue BootToCore::Unsupported
    warn "MASTODON_COPY_BOUNDARY=" + JSON.generate(owner: method.owner.name, name: method.name,
      original_name: method.original_name, source_location: method.source_location, stack: caller.first(8))
    raise
  end
end)

targets = {
  "enum" => ["LoginActivity", "app/models/login_activity.rb"],
  "attribute" => ["User", "app/models/user.rb"],
  "delegate" => ["StatusEdit", "app/models/status_edit.rb"],
  "settings" => ["UserSettings", "app/models/user_settings.rb"],
  "namespaced_attribute" => ["Fasp::Capability", "app/models/fasp/capability.rb"],
  "fasp_contract" => ["Fasp::Capability", "app/models/fasp/capability.rb"],
  "safe_delegate" => ["WebPushRequest", "app/lib/web_push_request.rb"],
  "prefixed_delegate" => ["AccountSuggestions::Suggestion", "app/models/account_suggestions/suggestion.rb"],
  "namespace_attributes" => ["TranslationService::Translation", "app/lib/translation_service/translation.rb"],
  "media_delegate" => ["StatusEdit::PreservedMediaAttachment", "app/models/status_edit.rb"],
  "policy_generated" => ["InteractionPolicy::SubPolicy", "app/lib/interaction_policy.rb"],
  "policy_plain" => ["InteractionPolicy::SubPolicy", "app/lib/interaction_policy.rb"],
  "keyword_message" => ["Admin::SystemCheck::Message", "app/lib/admin/system_check/message.rb"]
}

if lane == "full_capture"
  BootToCore.capture { require File.join(app, "config/environment") }
else
  # Capture before first relevant app declaration, but not unrelated gem boot.
  # Keep capture on through calls into gem generators until this class ends.
  if %w[enum_generation delegate_generation].include?(lane)
    # Additional narrow controls at the ORIGINAL declarations in this exact pin.
    # No DSL is replayed. These exclude the earlier, separately measured wrappers.
    path, lines = lane == "enum_generation" ? ["app/models/login_activity.rb", 21..21] : ["app/models/status_edit.rb", 41..42]
    trace = TracePoint.new(:line) do |event|
      BootToCore.active = lines.cover?(event.lineno) if event.path == File.join(app, path)
    end
    trace.enable
  elsif targets.key?(lane)
    name, path = targets.fetch(lane)
    trace = TracePoint.new(:class, :end) do |event|
      if event.path == File.join(app, path) && event.self.name == name
        BootToCore.active = event.event == :class
      end
    end
    trace.enable
  end
  begin
    require File.join(app, "config/environment")
  ensure
    trace&.disable
    BootToCore.active = false
  end
end
booted = true

roots = case lane
when "ordinary", "full_capture"
  { HttpSignatureDraft => [:request_target] }
when "accessor"
  { ResponseWithLimit => [:response, :limit] }
when "constant"
  { ASCIIFolding => [:fold] }
when "enum", "enum_root_only", "enum_generation"
  { LoginActivity => [:otp?] }
when "attribute"
  BootToCore.capture { User.define_attribute_methods }
  { User => [:external, :external=] }
when "attribute_root_only"
  User.define_attribute_methods
  { User => [:external, :external=] }
when "delegate", "delegate_root_only", "delegate_generation"
  { StatusEdit => [:local?] }
when "settings"
  { UserSettings => [:always_send_emails] }
when "namespaced_attribute"
  BootToCore.capture { Fasp::Capability.define_attribute_methods }
  { Fasp::Capability => [:enabled, :enabled=] }
when "namespaced_root_only"
  { Fasp::Capability => [:enabled, :enabled=] }
when "fasp_contract"
  BootToCore.capture { Fasp::Capability.define_attribute_methods }
  { Fasp::Capability => %i[id id= version version= enabled enabled=] }
when "safe_delegate"
  { WebPushRequest => %i[standard endpoint key_auth key_p256dh web_push_subscription legacy] }
when "prefixed_delegate"
  { AccountSuggestions::Suggestion => %i[account account= account_id sources sources=] }
when "namespace_attributes"
  { TranslationService::Translation => %i[text text= provider provider= detected_source_language detected_source_language=] }
when "media_delegate"
  { StatusEdit::PreservedMediaAttachment => %i[id local? media_attachment media_attachment= description description=] }
when "policy_generated"
  { InteractionPolicy::SubPolicy => %i[public? followers? following? disabled? missing?] }
when "policy_plain"
  { InteractionPolicy::SubPolicy => %i[missing?] }
when "keyword_message"
  { Admin::SystemCheck::Message => %i[key value action critical] }
else
  raise "unknown lane #{lane}"
end
warn "MASTODON_ROOTS=" + JSON.generate(roots.flat_map do |owner, names|
  names.map do |name|
    method = owner.instance_method(name)
    { owner: owner.name, method: name, definition_owner: method.owner.name,
      source_location: method.source_location, parameters: method.parameters,
      captured: BootToCore.records.key?([method.owner, name]) }
  end
end)
signatures = { "ordinary" => ["ordinary.rbs"], "accessor" => ["accessor.rbs"], "constant" => ["constant.rbs"] }
%w[fasp_contract safe_delegate prefixed_delegate namespace_attributes media_delegate policy_generated policy_plain keyword_message].each do |name|
  signatures[name] = ["generated.rbs"]
end
BootToCore.input(roots: roots, signatures: signatures.fetch(lane, []))
