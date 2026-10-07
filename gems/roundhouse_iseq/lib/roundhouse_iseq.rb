# frozen_string_literal: true

require_relative "roundhouse_iseq/version"
require_relative "roundhouse_iseq/compiler"
require_relative "roundhouse_iseq/loader"

module RoundhouseIseq
  # MRI-only delivery for Roundhouse-lowered Ruby: compile units to
  # InstructionSequence binaries, load them by manifest key.
end
