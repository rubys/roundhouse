# The fractal runtime against plain Ruby, driven by tests/fractal_runtime.rs.
#
# FRACTAL_APP is a fractal build of the app in that test. This file
# stands in for Rails: it gives the runtime an application root, a
# `to_prepare` and a Zeitwerk-shaped `on_load`, loads the generated
# initializer, then loads each class and runs its on_load callbacks the
# way an autoload would. FRACTAL_MODE picks the checks: plain, verify
# (FRACTAL_VERIFY=1) or strict (FRACTAL_STRICT=1).

require "minitest/autorun"
require "pathname"

APP = ENV.fetch("FRACTAL_APP")
FRAMEWORK = begin
  require "active_record"
  true
rescue LoadError
  false
end
MODE = ENV.fetch("FRACTAL_MODE")

module Rails
  Loader = Struct.new(:callbacks) do
    def on_load(cpath, &block)
      (callbacks[cpath] ||= []) << block
    end
  end
  Config = Struct.new(:prepares) do
    def to_prepare(&block)
      prepares << block
    end
  end
  Application = Struct.new(:root, :config)
  Autoloaders = Struct.new(:main)

  def self.application
    @application ||= Application.new(Pathname.new(APP), Config.new([]))
  end

  def self.autoloaders
    @autoloaders ||= Autoloaders.new(Loader.new({}))
  end
end

def path(file) = File.join(APP, file)

# What an autoload does: load the file, then run the callbacks.
def autoload_class(cpath, file)
  load path(file)
  klass = Object.const_get(cpath)
  Rails.autoloaders.main.callbacks.fetch(cpath, []).each { |callback| callback.call(klass, path(file)) }
  klass
end

# Edited after the build: its variant must not install.
File.write(path("app/models/edited.rb"), File.read(path("app/models/edited.rb")) + "# edited\n")

load path("config/initializers/fractal.rb")

LOADS = [
  # Before Plain, which their guards name: they wait for it to load.
  %w[Guarded app/models/guarded.rb],
  %w[GuardedOk app/models/guarded_ok.rb],
  %w[Plain app/models/plain.rb],
  %w[Hidden app/models/hidden.rb],
  %w[Admin::Report app/models/admin/report.rb],
  %w[Frozen app/models/frozen.rb],
  %w[Sing app/models/sing.rb],
  %w[Edited app/models/edited.rb],
  %w[Base app/models/base.rb],
  %w[Child app/models/child.rb],
  %w[Hooked app/models/hooked.rb],
  %w[Raiser app/models/raiser.rb],
  %w[Counter app/models/counter.rb],
  %w[Late app/models/late.rb],
  %w[GuardedLate app/models/guarded_late.rb],
].freeze

if MODE == "strict"
  class StrictTest < Minitest::Test
    def test_a_declined_variant_raises_at_load
      error = assert_raises(Fractal::Declined) { autoload_class("Patched", "app/models/patched.rb") }
      assert_match(/Patched#label/, error.message)
    end
  end
else
  LOADS.each { |cpath, file| autoload_class(cpath, file) }
  autoload_class("Patched", "app/models/patched.rb")
  if FRAMEWORK
    autoload_class("Record", "app/models/record.rb")
    autoload_class("Current", "app/models/current.rb")
  end
  # Loaded some other way than the autoloader: `to_prepare` catches it.
  load path("app/models/eager.rb")
end
# Tests run in random order, and a reload in one installs what is ready.
AT_LOAD = Fractal.status.dup.freeze

def status(prefix)
  Fractal.status.find { |key, _| key.start_with?(prefix) }&.last or raise "no variant #{prefix}"
end

if MODE == "plain"
  class InstallTest < Minitest::Test
    def test_an_installed_variant_answers
      assert_equal [:installed, nil], status("Plain#label")
      assert_equal "variant", Plain.new.label
    end

    def test_it_keeps_owner_file_and_line
      method = Plain.instance_method(:label)
      assert_equal Plain, method.owner
      assert_equal [path("app/models/plain.rb"), 2], method.source_location
    end

    def test_a_backtrace_points_at_the_apps_line
      assert_equal [:installed, nil], status("Raiser#boom")
      error = assert_raises(RuntimeError) { Raiser.new.boom }
      assert_equal "variant", error.message
      location = error.backtrace_locations.first
      assert_equal [path("app/models/raiser.rb"), 3], [location.path, location.lineno]
    end

    def test_it_keeps_visibility
      assert_equal [:installed, nil], status("Hidden#secret")
      assert Hidden.private_method_defined?(:secret)
      assert_equal "variant", Hidden.new.reveal
    end

    def test_it_keeps_lexical_nesting
      assert_equal [:installed, nil], status("Admin::Report#title")
      assert_equal "variant-report", Admin::Report.new.title
    end

    def test_it_keeps_the_frozen_string_literal_setting
      assert_equal "variant", Frozen.new.text
      assert Frozen.new.text.frozen?
      refute Plain.new.label.frozen?
    end

    def test_singleton_methods
      assert_equal %w[variant variant], [Sing.make, Sing.other]
      assert_equal [:installed, nil], status("Sing.make")
      assert_equal [:installed, nil], status("Sing.other")
    end

    def test_super_reaches_the_superclass
      assert_equal "base-variant", Child.new.label
    end

    def test_a_prepended_override_declines
      state, reason = status("Patched#label")
      assert_equal :declined, state
      assert_match(/owned by/, reason)
      assert_equal "original!", Patched.new.label
    end

    def test_a_file_changed_since_the_build_declines
      assert_equal [:declined, "app/models/edited.rb changed since the build"], status("Edited#label")
      assert_equal "original", Edited.new.label
    end

    def test_pass_guards_decide
      assert_equal [:installed, nil], status("GuardedOk#label")
      state, reason = status("Guarded#label")
      assert_equal :declined, state
      assert_match(%r{Plain#label is defined at .*/plain.rb:2, not .*/plain.rb:3}, reason)
      assert_equal "original", Guarded.new.label
    end

    def test_framework_hooks_that_do_nothing_on_redefinition_do_not_decline
      skip "activerecord is not installed" unless FRAMEWORK
      assert_equal [:installed, nil], status("Record#label")
      assert_equal [:installed, nil], status("Record.make")
      assert_equal %w[variant variant], [Record.allocate.label, Record.make]
      assert_equal [:installed, nil], status("Current#label")
      # The class-level delegator Rails generated still reaches it.
      assert_equal "variant", Current.label
    end

    def test_a_method_added_hook_declines
      state, reason = status("Hooked#label")
      assert_equal :declined, state
      assert_match(/method_added hook in #<Class:Hooked>/, reason)
      assert_equal "original", Hooked.new.label
    end

    # One test, because each prepare installs whatever is ready.
    def test_prepare_installs_what_the_autoloader_did_not
      at_load = ->(prefix) { AT_LOAD.find { |key, _| key.start_with?(prefix) }.last }
      assert_equal :pending, at_load.("Eager#label").first
      assert_equal :pending, at_load.("Late#label").first
      assert_equal [:pending, "waiting for Eager to load"], at_load.("GuardedLate#label")
      Rails.application.config.prepares.each(&:call)
      assert_equal [:installed, nil], status("Eager#label")
      assert_equal "variant", Eager.new.label
      assert_equal [:installed, nil], status("GuardedLate#label")
      # Late's method is in a reopening not loaded yet.
      assert_equal [:pending, "Late#label is not defined yet"], status("Late#label")
      load path("app/models/late/reopened.rb")
      Rails.application.config.prepares.each(&:call)
      assert_equal [:installed, nil], status("Late#label")
      assert_equal "variant", Late.new.label
    end

    def test_a_reloaded_class_gets_its_variant_again
      Object.send(:remove_const, :Plain)
      reloaded = autoload_class("Plain", "app/models/plain.rb")
      assert_equal "variant", reloaded.new.label
    end
  end
end

if MODE == "verify"
  class VerifyTest < Minitest::Test
    def test_a_replayable_variant_is_compared_with_the_original
      error = assert_raises(Fractal::Mismatch) { Plain.new.label }
      assert_match(/original gave "original", the variant "variant"/, error.message)
      assert_equal 1, Fractal.calls.values_at(*Fractal.calls.keys.grep(/\APlain#label/)).sum
    end

    def test_exceptions_are_compared_by_class_and_message
      error = assert_raises(Fractal::Mismatch) { Raiser.new.boom }
      assert_match(/original gave RuntimeError: original, the variant RuntimeError: variant/, error.message)
    end

    def test_a_variant_of_a_method_that_mutates_self_is_not_compared
      assert_equal "variant", Counter.new.bump
      assert_empty Fractal.calls.keys.grep(/\ACounter/)
    end
  end
end
