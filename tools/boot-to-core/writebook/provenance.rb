# frozen_string_literal: true

# Diagnostic-only wrappers: delegate unchanged, re-raise unchanged, never admit
# a failed definition. Do not wrap eval or obtain its native event binding.
require "json"

module WritebookCaptureProvenance
  def record_eval(owner, source, path, first_line)
    super
  rescue BootToCore::Unsupported => error
    nodes = Prism.parse(source).value.statements.body.map { |node| node.class.name }
    warn "EVAL_PROVENANCE: owner=#{owner.name.inspect} path=#{path} line=#{first_line} nodes=#{nodes}"
    warn error.backtrace.first(16).join("\n")
    raise
  end

  def source_record(method)
    super
  rescue BootToCore::Unsupported => error
    warn "METHOD_PROVENANCE: owner=#{method.owner} name=#{method.name} original=#{method.original_name} source=#{method.source_location.inspect}"
    warn error.backtrace.first(16).join("\n")
    raise
  end

  def export(*args, **options)
    super
  ensure
    # Metadata only: do not expose bindings, captured values or mutable heaps.
    records = BootToCore.records.values
    opaque = records.select { |record| record.respond_to?(:reason) && record.reason }
    metadata = {
      definitions: records.length, opaque_definitions: opaque.length,
      opaque_reasons: opaque.map { |record| [record.origin, record.reason] }.tally.map { |identity, count| { identity: identity, count: count } },
      opaque_events: BootToCore.events.select { |event| event[:kind] == "opaque_eval" }.map { |event| event[:reason] }.tally,
      observer_failures: defined?(NativeDefineMethodObserver) ? NativeDefineMethodObserver.failures.map { |owner, name, error| { owner: owner.name, method: name, kind: error.class.name, message: error.message } } : [],
      scalar_constants: BootToCore.events.select { |event| event[:kind] == "scalar_constant" },
      immutable_constants: BootToCore.events.select { |event| event[:kind] == "immutable_constant" }
    }
    warn "WRITEBOOK_CAPTURE_METADATA: #{JSON.generate(metadata)}"
  end
end

BootToCore.singleton_class.prepend(WritebookCaptureProvenance)
