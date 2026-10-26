# frozen_string_literal: true

require_relative "lib/roundsnap/version"

Gem::Specification.new do |s|
  s.name          = "roundsnap"
  s.version       = Roundsnap::VERSION
  s.summary       = "Compile lowered Ruby units to MRI ISeq binaries and load them by manifest key"
  s.description   = <<~DESC
    Roundsnap is Roundhouse's MRI delivery vehicle — the Bootsnap analogue
    for Roundhouse-emitted trees: compile source units to
    RubyVM::InstructionSequence binaries with emitted file locations
    and source-map sidecars,
    then require them by logical key from a manifest.
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
  s.executables   = ["roundsnap-compile"]
  s.require_paths = ["lib"]
end
