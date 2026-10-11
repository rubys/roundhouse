# frozen_string_literal: true

BootToCore.capture { require_relative "fixture" }
BootToCore.input(
  roots: {
    EvalProduct => %i[compute fallback],
    LeftProduct => %i[label rename advance current flag selected],
    RightProduct => %i[label rename advance current flag selected]
  },
  signatures: ["contract.rbs"]
)
