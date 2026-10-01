#!/usr/bin/env ruby
require_relative "evidence"

name, root, output = ARGV
abort "usage: ruby fixtures/gem-capabilities/build.rb roundhouse|spinel SOURCE NEW_OUTPUT" unless Evidence::ORIGINS.key?(name) && root && output
root, output = [root, output].map { |path| File.expand_path(path) }
abort "output already exists" if File.exist?(output)
source = Evidence.source(root, name)
pin = name == "roundhouse" ? Evidence::ROUNDHOUSE : Evidence::SPINEL
abort "wrong/dirty compiler source or origin: #{source}" unless Evidence.valid_source?(source, name)
FileUtils.mkdir_p(output)
receipt = {"source_root" => root, "source" => source, "ruby" => RUBY_DESCRIPTION,
           "ruby_executable" => RbConfig.ruby, "build" => []}
if name == "roundhouse"
  target = File.join(output, "cargo-target")
  env = {"CARGO_TARGET_DIR" => target, "ROUNDHOUSE_COMMIT" => pin}
  receipt["build"] << Evidence.capture("cargo", "build", "--locked", "--bin", "roundhouse", dir: root, env: env)
  receipt["binary"] = File.join(target, "debug/roundhouse")
  receipt["cargo_lock_sha256"] = Evidence.sha(File.join(root, "Cargo.lock"))
else
  receipt["build"] << Evidence.capture("make", "deps", dir: root)
  # Source cleanliness does not invalidate ignored objects left by an older build.
  receipt["build"] << Evidence.capture("make", "-B", "-j4", dir: root) if receipt["build"].last["exit"] == 0
  receipt["binary"] = File.join(root, "bin/spinel")
  receipt["parser_sources"] = Evidence.files(File.join(root, "vendor"))
end
receipt["toolchains"] = %w[cargo rustc cc make].to_h { |tool| [tool, Evidence.capture(tool, "--version", dir: root)] }
receipt["binary_sha256"] = Evidence.sha(receipt["binary"])
receipt["version"] = Evidence.capture(receipt["binary"], "--version", dir: root) if receipt["binary_sha256"]
receipt["source_after"] = Evidence.source(root, name)
Evidence.write(File.join(output, "receipt.json"), receipt)
abort "build/provenance failed; inspect #{output}/receipt.json" unless Evidence.provenance(receipt, receipt["source_after"], name) == "verified"
puts "VERIFIED #{name} #{pin} #{receipt['binary_sha256']}"
