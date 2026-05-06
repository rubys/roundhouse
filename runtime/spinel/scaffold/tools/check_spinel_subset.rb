#!/usr/bin/env ruby
# Linter that enforces the Spinel-subset gradient on this fixture.
#
# Walks runtime/, app/, config/ (the production code paths — tests and
# tools themselves are exempted because they may exercise patterns we
# forbid in production code). For each .rb file, scans line-by-line for
# patterns that Spinel will reject. Exits non-zero if any matches.
#
# Each iteration of the spinel-blog tightening adds rules; older rules
# stay. The RULES table is the source of truth for "what does
# iteration-N forbid."

ROOT = File.expand_path("..", __dir__)
SCAN_DIRS = %w[runtime app config].freeze

# Pattern => human-readable rule description.
# Anchored at word boundaries where possible to avoid false positives
# inside strings/comments. (A real linter would need an actual parser
# for full accuracy; for the spinel-subset gradient, line-level grep is
# precise enough — false positives can be revisited per-rule when they
# arise.)
RULES = {
  /\binstance_variable_get\b/  => "no instance_variable_get (use explicit accessor)",
  /\binstance_variable_set\b/  => "no instance_variable_set (use explicit assignment)",
  /\bdefine_method\b/          => "no define_method (use def at class-definition time)",
  /\bmethod_missing\b/         => "no method_missing (Spinel rejects)",
  /\b(class|instance|module)_eval\b/ => "no class_eval/instance_eval/module_eval",
  /(?<!\w)eval\s*\(/           => "no eval (Spinel rejects)",
  /\.\s*send\s*\(/             => "no .send( — call the method directly",
  /\b__send__\b/               => "no __send__ — call the method directly",
  /\bThread\b(?!safe)/         => "no Thread (Spinel has no thread support)",
  /\bMutex\b/                  => "no Mutex (Spinel has no thread support)",
}.freeze

violations = []

def each_ruby_file(root, scan_dirs)
  scan_dirs.each do |dir|
    full = File.join(root, dir)
    next unless File.directory?(full)
    Dir[File.join(full, "**", "*.rb")].sort.each do |path|
      yield path
    end
  end
end

each_ruby_file(ROOT, SCAN_DIRS) do |path|
  rel = path.sub(/\A#{Regexp.escape(ROOT)}\//, "")
  File.foreach(path).with_index(1) do |line, lineno|
    # Strip comments (very rough — doesn't handle # inside strings, but
    # comments are by far the more common false-positive source).
    code = line.sub(/(^|[^"'])#.*/, '\1')
    RULES.each do |pattern, rule|
      next unless code =~ pattern
      col = $~.begin(0) + 1
      violations << "#{rel}:#{lineno}:#{col}: #{rule}"
    end
  end
end

if violations.empty?
  files_scanned = 0
  each_ruby_file(ROOT, SCAN_DIRS) { |_| files_scanned += 1 }
  puts "spinel-subset OK (#{files_scanned} file(s) clean)"
  exit 0
else
  warn "spinel-subset violations (#{violations.size}):"
  violations.each { |v| warn "  #{v}" }
  exit 1
end
