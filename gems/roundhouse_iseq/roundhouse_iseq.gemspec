# frozen_string_literal: true

require_relative "lib/roundhouse_iseq/version"

Gem::Specification.new do |s|
  s.name          = "roundhouse_iseq"
  s.version       = RoundhouseIseq::VERSION
  s.summary       = "Compile lowered Ruby units to MRI ISeq binaries and load them by manifest key"
  s.description   = <<~DESC
    Roundhouse's MRI delivery vehicle: compile source units to
    RubyVM::InstructionSequence binaries with original file metadata,
    then require them by logical key from a manifest. Replaces
    bootsnap-style compile caching for Roundhouse-emitted trees.
    MRI only — not a Bootsnap.setup drop-in for unmodified Rails.
  DESC
  s.authors       = ["Roundhouse contributors"]
  s.email         = ["roundhouse@example.com"]
  s.homepage      = "https://github.com/rubys/roundhouse"
  s.license       = "MIT"

  s.required_ruby_version = ">= 3.2.0"
  s.files = Dir.chdir(__dir__) do
    Dir["lib/**/*.rb", "exe/*", "README.md", "LICENSE*", "*.gemspec"].select { |f| File.file?(f) }
  end
  s.bindir        = "exe"
  s.executables   = ["roundhouse-iseq-compile"]
  s.require_paths = ["lib"]
end
