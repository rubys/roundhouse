# bench/lobsters/spec-shim/rails_helper.rb — boot the roundhouse-emitted
# lobsters tree instead of Rails, so upstream lobsters' OWN RSpec suite
# runs against the emit.
#
# The suite is a far finer oracle than route parity: 354 model examples
# each assert something specific about behavior, so every failure names
# a construct we haven't modeled. Their specs are the worklist generator.
#
# Normally invoked via scripts/lobsters-specs, which emits a fresh tree,
# picks the Ruby the checkout pins, and clusters the failures. To drive
# it by hand, from THIS directory:
#
#   EMIT_TREE=/path/to/emit LOBSTERS_SRC=~/git/lobsters \
#     BUNDLE_GEMFILE=~/git/lobsters/Gemfile \
#     bundle exec rspec -I . ~/git/lobsters/spec/models/story_spec.rb
#
# NEVER add lobsters' spec/ to the load path (`-I …/lobsters/spec`).
# Their spec/rails_helper.rb would win the `require "rails_helper"` race,
# real Rails would boot, and the suite would pass while testing nothing
# of ours. Verified: it reports 14 examples, 0 failures and looks exactly
# like success.
#
# Note what that means for the canary below — in the trap, THIS FILE NEVER
# RUNS, so no check written here can catch it. The check that works lives
# in the caller: the shim writes $CANARY_OUT, and scripts/lobsters-specs
# treats the file's ABSENCE as failure. "Never ran" has to be as loud as
# "ran and was wrong", or the quietest failure is the one that passes.

require "json"

EMIT = ENV["EMIT_TREE"] or abort "rails_helper shim: EMIT_TREE must point at an emitted lobsters tree"
LOBSTERS = ENV["LOBSTERS_SRC"] || File.expand_path("~/git/lobsters")

# lobsters' own spec_helper, by absolute path — see the load-path note above.
require File.join(LOBSTERS, "spec", "spec_helper")

# HARNESS COMPENSATION (gap): app/models/sponge.rb references Net::
# constants but the emit never requires net/http. Reported by
# scripts/lobsters-specs as a compensation, not silently absorbed —
# a number propped up by the harness is not a number about the emit.
require "net/http"

ENV["BLOG_DB"] = ":memory:"
require File.join(EMIT, "main")
Main.configure_default_adapter!

# ── Canary ───────────────────────────────────────────────────────────
# Fatal, not advisory. The whole suite is meaningless if it ran against
# real Rails, and that mistake is one stray -I away at any time.
#
# Writing $CANARY_OUT is the load-bearing half: it is the only evidence
# the caller has that this file ran at all.
valid_from = Story.instance_method(:valid?).source_location&.first.to_s
CANARY = {
  "rails_booted" => !!defined?(Rails::Engine),
  "valid_source" => valid_from,
  "valid_from_emit" => valid_from.start_with?(File.expand_path(EMIT)),
}
CANARY["ok"] = !CANARY["rails_booted"] && CANARY["valid_from_emit"]
File.write(ENV["CANARY_OUT"], JSON.generate(CANARY)) if ENV["CANARY_OUT"]

unless CANARY["ok"]
  abort <<~MSG
    rails_helper shim: CANARY FAILED — this run would not have tested the emit.
      real Rails booted:      #{CANARY["rails_booted"]}
      Story#valid? loaded from: #{valid_from.empty? ? "(unknown)" : valid_from}
      expected it under:        #{File.expand_path(EMIT)}
    Most likely cause: lobsters' spec/ reached the load path and its own
    rails_helper.rb answered the require first.
  MSG
end

# HARNESS COMPENSATION (by design): spec and factory code is untranspiled
# plain Ruby. The emit path rewrites `n.days` → Duration.days(n) at
# transpile time, so the shared runtime deliberately never reopens
# Numeric ([[feedback_runtime_must_be_statically_resolvable]]) — which
# leaves the spec-facing monkeypatch to the harness, where it belongs.
class Numeric
  %i[seconds minutes hours days weeks fortnights months years].each do |unit|
    define_method(unit) { ActiveSupport::Duration.public_send(unit, self) }
    define_method(unit.to_s.chomp("s")) { ActiveSupport::Duration.public_send(unit, self) }
  end
end

require "factory_bot"
require "faker"
FactoryBot.definition_file_paths = [File.join(LOBSTERS, "spec", "factories")]
FactoryBot.find_definitions

RSpec.configure do |config|
  config.include FactoryBot::Syntax::Methods

  # Fresh in-memory DB per example (stand-in for transactional fixtures):
  # reopen :memory: and replay the schema DDL, mirroring
  # Main.configure_default_adapter!.
  config.before(:each) do
    SqliteAdapter.configure(":memory:")
    ActiveRecord.adapter = SqliteAdapter
    Schema.statements.each { |sql| SqliteAdapter.execute_ddl(sql) }
  end
end
