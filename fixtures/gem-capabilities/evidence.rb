require "json"
require "open3"
require "digest"
require "fileutils"
require "rbconfig"
require "time"

module Evidence
  ROUNDHOUSE = "f3d192810b53c61a73a52017e23d67df80265f72"
  SPINEL = "7147b681fd885ac1038f78474b010f2ab2d58402"
  ORIGINS = {"roundhouse" => "https://github.com/rubys/roundhouse", "spinel" => "https://github.com/matz/spinel"}.freeze

  def self.capture(*argv, dir:, env: {})
    started = Time.now.utc.iso8601
    stdout, stderr, status = Open3.capture3(env, *argv, chdir: dir)
    {"command" => argv, "cwd" => dir, "env" => env, "started" => started,
     "exit" => status.exitstatus, "signal" => status.termsig, "stdout" => stdout, "stderr" => stderr}
  rescue Errno::ENOENT => error
    {"command" => argv, "cwd" => dir, "started" => started, "exit" => nil,
     "stdout" => "", "stderr" => error.message, "unavailable" => true}
  end

  def self.sha(path)
    Digest::SHA256.file(path).hexdigest if path && File.file?(path)
  end

  def self.files(root)
    Dir.glob("#{root}/**/*", File::FNM_DOTMATCH).select { |p| File.file?(p) && !p.include?("/.git/") }
      .sort.to_h { |p| [p.delete_prefix("#{root}/"), sha(p)] }
  end

  def self.source(root, name)
    pin = name == "roundhouse" ? ROUNDHOUSE : SPINEL
    paths = name == "roundhouse" ? [".", ":(exclude)fixtures/gem-capabilities", ":(exclude)scripts/check-gem-capabilities.rb"] : ["."]
    commands = {
      "head" => ["git", "rev-parse", "HEAD"],
      "origin" => ["git", "remote", "get-url", "origin"],
      "status" => ["git", "status", "--porcelain", "--untracked-files=all"],
      "diff" => ["git", "diff", "HEAD", "--", "."],
      "compiler_pin" => ["git", "rev-parse", pin],
      "compiler_diff" => ["git", "diff", pin, "--", *paths],
      "tree" => ["git", "rev-parse", "HEAD^{tree}"]
    }
    commands["upstream"] = ["git", "remote", "get-url", "upstream"] if name == "roundhouse"
    commands.to_h do |key, argv|
      result = capture(*argv, dir: root)
      raise "cannot inspect #{root}: #{result}" unless result["exit"] == 0
      [key, result["stdout"].chomp]
    end
  end

  def self.valid_source?(source, name)
    pin = name == "roundhouse" ? ROUNDHOUSE : SPINEL
    source.is_a?(Hash) && %w[head tree origin status diff compiler_pin compiler_diff].all? { |key| source[key] } &&
      source["head"].match?(/\A[0-9a-f]{40}\z/) && source["tree"].match?(/\A[0-9a-f]{40}\z/) &&
      (name == "roundhouse" || source["head"] == pin) && source["compiler_pin"] == pin && source["compiler_diff"] == "" &&
      source.fetch(name == "roundhouse" ? "upstream" : "origin", "").delete_suffix(".git") == ORIGINS.fetch(name) &&
      source.fetch("status", "missing").lines.all? { |line| name == "roundhouse" && line.match?(/\A.{3}(fixtures\/gem-capabilities\/|scripts\/check-gem-capabilities\.rb$)/) }
  end

  def self.provenance(receipt, actual, name)
    return "provenance-missing" unless receipt.is_a?(Hash) && %w[source source_after source_root binary binary_sha256 build version].all? { |key| receipt[key] }
    pin = name == "roundhouse" ? ROUNDHOUSE : SPINEL
    valid = valid_source?(receipt["source"], name) && valid_source?(receipt["source_after"], name) && valid_source?(actual, name) &&
      receipt["source"]["origin"] == actual["origin"] && receipt["source_after"]["origin"] == actual["origin"] &&
      receipt["build"].is_a?(Array) && receipt["build"].all? { |step| step.is_a?(Hash) && step["exit"] == 0 } && !receipt["build"].empty? &&
      (name != "spinel" || receipt["build"].any? { |step| step["command"] == ["make", "-B", "-j4"] && step["cwd"] == receipt["source_root"] }) &&
      receipt["version"].is_a?(Hash) && receipt["version"]["exit"] == 0 && receipt["version"]["stdout"].to_s.include?(pin[0, 8]) &&
      sha(receipt["binary"]) == receipt["binary_sha256"] && File.executable?(receipt["binary"])
    valid ? "verified" : "provenance-mismatch"
  end

  def self.observe_tool(receipt, name)
    return nil unless receipt.is_a?(Hash) && receipt["source_root"] && File.directory?(receipt["source_root"])
    source(receipt["source_root"], name)
  end

  # Pass is scoped to this stage. A successful check or compile is not a runtime pass.
  def self.classify(result, provenance:, artifacts: [], expected: nil, failure: "runtime-fail")
    return provenance unless provenance == "verified"
    return "not-run" unless result
    return "environment-blocked" if result["unavailable"]
    return failure unless result["exit"] == 0
    return "emission-missing" unless artifacts.all? { |path| File.file?(path) }
    return "contract-fail" if expected && result["stdout"] != expected
    "pass"
  end

  def self.write(path, value)
    File.write(path, JSON.pretty_generate(value) + "\n")
  end
end
