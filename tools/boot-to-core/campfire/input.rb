# frozen_string_literal: true

# Run only against the disposable, pinned app, through probe.rb's clean env.
require "json"
app = File.realpath(ENV.fetch("CORE_RUBY_APP"))
lane = ENV.fetch("CAMPFIRE_CORE_LANE")
phase = "load_dependencies_outside_capture"
at_exit do
  opaque = BootToCore.records.values.select(&:reason)
  warn "PROVENANCE #{JSON.generate(opaque_definitions: opaque.length,
    opaque_reasons: opaque.group_by { |record| [record.origin, record.reason] }.map { |(origin, reason), records| { origin: origin, reason: reason, count: records.length } },
    opaque_sites: opaque.map { |record| { owner: record.owner.name, method: record.name, source: record.definition.source_location, reason: record.reason } },
    event_counts: BootToCore.events.group_by { |event| event[:kind] }.transform_values(&:length),
    scalar_constants: BootToCore.events.select { |event| event[:kind] == "scalar_constant" },
    observer_failures: NativeDefineMethodObserver.failures.map { |owner, name, error| { owner: owner.name, method: name, kind: error.class.name, message: error.message } })}"
end
begin
  require File.join(app, "config/application")
  if %w[whole_boot whole_attributes].include?(lane)
    phase = "initialize_inside_capture"
    BootToCore.capture do
      Rails.application.initialize!
      phase = "eager_load_inside_capture"
      Rails.application.eager_load!
      if lane == "whole_attributes"
        phase = "attributes_inside_capture"
        [User, Room, Message].each(&:define_attribute_methods)
      end
    end
  else
    phase = "initialize_outside_capture"
    Rails.application.initialize!
    if %w[user_load enum_capture].include?(lane)
      phase = "user_model_load_inside_capture"
      BootToCore.capture do
        User
        if lane == "user_load"
          phase = "user_attributes_inside_capture"
          User.define_attribute_methods
        end
      end
    else
      phase = "eager_load_outside_capture"
      Rails.application.eager_load!
      if lane == "attribute_ready"
        phase = "schema_and_primary_key_outside_capture"
        User.columns
        User.primary_key
        User.attribute_types
        User.send(:_default_attributes)
      end
      if %w[attribute attribute_ready].include?(lane)
        warn "ATTRIBUTE_BEFORE #{User.send(:attribute_methods_generated?).inspect}"
        phase = "user_attributes_inside_capture"
        BootToCore.capture { warn "ATTRIBUTE_GENERATED #{User.define_attribute_methods.inspect}" }
      elsif %w[application initials].include?(lane)
        phase = "attribute_generation_outside_capture"
        [User, Room, Message].each(&:define_attribute_methods)
      end
    end
  end
rescue StandardError, LoadError, SyntaxError => error
  warn "PHASE #{phase}"
  warn "CAPTURE_PROGRESS #{JSON.generate(events: BootToCore.events.length,
    user_name_bodies: BootToCore.records.values.select { |record| %i[name name=].include?(record.name) }.map { |record| { owner: record.owner.name, method: record.name, origin: record.origin } })}"
  warn error.full_message
  raise
end
warn "BOOT_COMPLETE #{JSON.generate(rails: Rails.version, ruby: RUBY_VERSION, capture_events: BootToCore.events.length)}"

roots = case lane
when "leaf"
  # Real notification default and real mention-template selectors. No app body
  # is replaced, copied by hand, given a stub getter, or monkey-patched.
  { Room => [:default_involvement], User => [:to_attachable_partial_path, :to_editor_content_attachment_partial_path] }
when "enum", "enum_capture"
  { User => [:active?] }
when "mention"
  value = User::Mentionable::MENTION_CONTENT_TYPE
  warn "MENTION_CONSTANT #{JSON.generate(owner: 'User::Mentionable', type: value.class.name, frozen: value.frozen?)}"
  { User => [:attachable_content_type] }
when "initials"
  { User => [:initials] }
when "platform"
  { ApplicationPlatform => [:ios?, :android?, :mobile?, :desktop?] }
when "sound"
  { Sound => [:name, :asset_path, :image, :text] }
when "attribute", "attribute_ready", "user_load"
  { User => [:name, :name=] }
else
  { User => [:name, :name=, :initials, :title, :active?, :valid?, :save],
    Room => [:name, :name=, :open?, :default_involvement],
    Message => [:plain_text_body, :content_type],
    RoomsController => [:index, :show, :destroy] }
end
warn "ROOT_EXPORT #{JSON.generate(roots.to_h { |owner, names| [owner.name, names] })}"
warn "ROOT_DEFINITIONS #{JSON.generate(roots.flat_map { |owner, names| names.map { |name|
  begin
    method = owner.instance_method(name)
    { root: "#{owner.name}##{name}", owner: method.owner.name, source: method.source_location }
  rescue NameError => error
    { root: "#{owner.name}##{name}", missing: error.message }
  end
} })}"
BootToCore.input(roots: roots)
