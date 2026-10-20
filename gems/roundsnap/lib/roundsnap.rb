# frozen_string_literal: true

require_relative "roundsnap/version"
require_relative "roundsnap/compiler"
require_relative "roundsnap/loader"
require_relative "roundsnap/source_map"

module Roundsnap
  # MRI-only delivery for Roundhouse-lowered Ruby: compile units to
  # InstructionSequence binaries, load them by manifest key.
  # Bootsnap analogue for Roundhouse-emitted trees.
end
