# frozen_string_literal: true

require_relative "rails_fixture"
callback = CallbackProduct._validation_callbacks.find { |entry| entry.kind == :before }.filter
BootToCore.input(
  roots: {
    RailsCoreProduct => %i[name name= name_describe title_describe],
    CallbackProduct => [:prime, callback, :review, :ledger, :probe_weight, :allowed?]
  },
  signatures: ["rails_core.rbs"]
)
