# frozen_string_literal: true

# Fractal runtime, written by `roundhouse --target fractal`.
#
# Each `Fractal.variant` below is a specialized definition of one method
# in a class the build selected. It is installed when its owner loads,
# and only if every assumption it was built under still holds:
#
# - the method is still the app's own definition, at its file and line,
#   with nothing having prepended, wrapped or redefined it;
# - the file is byte for byte the one the build read;
# - the owner has no `method_added` hook that installing would run;
# - each guard a pass added holds.
#
# Otherwise the app's method stays, and `Fractal.status` says why.
# Installing redefines the method in its own class, at its own nesting,
# file and line, with its own visibility: callers, `super`, backtraces
# and `instance_method(...).owner` see what they saw before.
#
#   Fractal.status          # { key => [:installed | :declined | :pending, reason] }
#   FRACTAL_STRICT=1        # raise instead of declining (for CI)
#   FRACTAL_VERIFY=1        # run original and variant, raise on a difference
#   Fractal.calls           # { key => calls }, under FRACTAL_VERIFY
require "zlib"

module Fractal
  Variant = Struct.new(
    :key, :owner, :method, :singleton, :file, :line, :file_size, :file_crc,
    :eval_line, :replayable, :guards, :source, keyword_init: true
  )

  # Framework hooks that defining a method again runs, and that do
  # nothing when the method already exists: Current attributes generate
  # a class-level delegator only for a method that has none, and Active
  # Record generates a relation delegator only for a class method named
  # like a Kernel method that Relation lacks. Any other hook declines.
  HARMLESS_HOOKS = {
    method_added: ["ActiveSupport::CurrentAttributes"],
    singleton_method_added: ["ActiveRecord::Scoping::Named::ClassMethods"],
  }.freeze

  class Declined < StandardError; end
  class Mismatch < StandardError; end

  @variants = []
  @status = {}
  @calls = Hash.new(0)
  @installed = {}.compare_by_identity
  @crcs = {}
  @root = Dir.pwd

  class << self
    attr_reader :variants, :status, :calls
    attr_accessor :root

    def variant(**attributes)
      variant = Variant.new(**attributes.merge(method: attributes.fetch(:method).to_sym))
      @variants << variant
      @status[variant.key] = [:pending, "#{variant.owner} has not loaded"]
      variant
    end

    # Hook the variants into a Rails app: install when an owner is
    # autoloaded or eager loaded (every time, so a reload gets them too),
    # and on each prepare for owners that were loaded some other way.
    # Installing at load, before the first call, means a JIT never
    # compiles the original only to throw the code away.
    def attach(app)
      @root = app.root.to_s
      owners, guarded = self.owners, guard_constants
      (owners | guarded).each do |constant|
        Rails.autoloaders.main.on_load(constant) do |klass, _path|
          install(klass, constant) if owners.include?(constant)
          install_loaded if guarded.include?(constant)
        end
      end
      app.config.to_prepare { Fractal.install_loaded }
    end

    def install_loaded
      owners.each do |owner|
        klass = loaded(owner)
        install(klass, owner) if klass
      end
    end

    # Install every variant of `owner` on `klass`, once per class object.
    # A variant stays pending while its method is not defined yet (a
    # later reopening defines it) or a constant a guard names has not
    # loaded; the next prepare, or that constant's load, tries again.
    def install(klass, owner = klass.name)
      done = (@installed[klass] ||= [])
      @crcs = {}
      @variants.each do |variant|
        next if variant.owner != owner || done.include?(variant.key)

        target = target(klass, variant)
        unless defines?(target, variant.method)
          @status[variant.key] = [:pending, "#{target}##{variant.method} is not defined yet"]
          next
        end
        waiting = variant.guards.map { |guard| guard[:constant] }.uniq.reject { |constant| loaded(constant) }
        unless waiting.empty?
          @status[variant.key] = [:pending, "waiting for #{waiting.join(", ")} to load"]
          next
        end

        done << variant.key
        reason = refusal(klass, variant)
        reason ||= define(klass, variant)
        if reason
          @status[variant.key] = [:declined, reason]
          raise Declined, "#{variant.key}: #{reason}" if ENV["FRACTAL_STRICT"] == "1"
        else
          @status[variant.key] = [:installed, nil]
        end
      end
    end

    # Run both and compare: the value, or the exception's class and
    # message. Public because the verify wrapper calls it from the owner.
    def compare(key, original, variant)
      @calls[key] += 1
      expected = outcome(&original)
      actual = outcome(&variant)
      unless same?(expected, actual)
        raise Mismatch, "#{key}: the original gave #{describe(expected)}, the variant #{describe(actual)}"
      end
      raise expected[1] if expected[0] == :raise

      expected[1]
    end

    private

    def owners
      @variants.map(&:owner).uniq
    end

    def guard_constants
      @variants.flat_map { |variant| variant.guards.map { |guard| guard[:constant] } }.uniq
    end

    # The constant if it is already loaded, without triggering autoload.
    def loaded(owner)
      owner.split("::").reduce(Object) do |namespace, name|
        return nil unless namespace.const_defined?(name, false) && !namespace.autoload?(name)

        namespace.const_get(name, false)
      end
    end

    def path(file)
      File.expand_path(file, @root)
    end

    def target(klass, variant)
      variant.singleton ? klass.singleton_class : klass
    end

    def refusal(klass, variant)
      file = path(variant.file)
      return "#{variant.file} is missing" unless File.file?(file)
      unless File.size(file) == variant.file_size && crc(file) == variant.file_crc
        return "#{variant.file} changed since the build"
      end

      target = target(klass, variant)
      reason = definition_refusal(target, variant.method, file, variant.line)
      return reason if reason

      hook = variant.singleton ? :singleton_method_added : :method_added
      unexpected = hooks(klass, hook).first
      return "#{klass} has a #{hook} hook in #{unexpected}, which installing would run" if unexpected
      if !variant.singleton && !klass.is_a?(Class) && module_function?(klass, variant.method)
        return "the definition is also #{klass}'s singleton method (module_function)"
      end

      variant.guards.each do |guard|
        reason = guard_refusal(guard)
        return reason if reason
      end
      nil
    end

    # The modules that define `hook` for `klass`, other than Ruby's own
    # no-op and the harmless framework ones.
    def hooks(klass, hook)
      default = hook == :method_added ? Module : BasicObject
      harmless = HARMLESS_HOOKS.fetch(hook).filter_map { |name| loaded(name) }
      klass.singleton_class.ancestors.select do |mod|
        next false if mod == default || harmless.any? { |known| mod == known || mod == known.singleton_class }

        mod.private_method_defined?(hook, false) || mod.method_defined?(hook, false)
      end
    end

    def crc(file)
      @crcs[file] ||= Zlib.crc32(File.binread(file))
    end

    def defines?(target, method)
      target.private_method_defined?(method, false) || target.method_defined?(method, false)
    end

    def definition_refusal(target, method, file, line)
      return "#{target}##{method} is not defined" unless defines?(target, method)

      current = target.instance_method(method)
      return "#{target}##{method} is owned by #{current.owner}" unless current.owner == target

      location = current.source_location
      if location.nil? || !same_file?(location[0], file) || location[1] != line
        return "#{target}##{method} is defined at #{location&.join(":") || "an unknown location"}, not #{file}:#{line}"
      end
      nil
    end

    def module_function?(mod, method)
      singleton = mod.singleton_class
      return false unless singleton.private_method_defined?(method) || singleton.method_defined?(method)

      singleton.instance_method(method).source_location == mod.instance_method(method).source_location
    end

    def guard_refusal(guard)
      case guard[:kind]
      when :defined_at
        constant = loaded(guard[:constant])
        return "#{guard[:constant]} is not loaded" unless constant

        target = guard[:singleton] ? constant.singleton_class : constant
        definition_refusal(target, guard[:method].to_sym, path(guard[:file]), guard[:line])
      else
        "unknown guard #{guard[:kind].inspect}"
      end
    end

    def same_file?(a, b)
      a == b || (File.exist?(a) && File.exist?(b) && File.realpath(a) == File.realpath(b))
    end

    # Define the variant in place of the original. Returns a reason when
    # it could not be, with the original left (or put back) in place.
    def define(klass, variant)
      target = target(klass, variant)
      visibility = visibility(target, variant.method)
      original = target.instance_method(variant.method)
      verbose, $VERBOSE = $VERBOSE, nil
      begin
        TOPLEVEL_BINDING.eval(variant.source, path(variant.file), variant.eval_line)
      ensure
        $VERBOSE = verbose
      end
      installed = target.instance_method(variant.method)
      unless installed.owner == target && installed.source_location == [path(variant.file), variant.line]
        restore(target, variant.method, original, visibility)
        return "the variant did not define #{target}##{variant.method} where expected"
      end
      wrap(target, variant, original, installed) if verify?(variant)
      target.send(visibility, variant.method)
      nil
    rescue ScriptError, StandardError => e
      restore(target, variant.method, original, visibility) if original
      "installing raised #{e.class}: #{e.message}"
    end

    def restore(target, method, original, visibility)
      target.define_method(method, original)
      target.send(visibility, method)
    end

    def visibility(target, method)
      if target.private_method_defined?(method, false) then :private
      elsif target.protected_method_defined?(method, false) then :protected
      else :public
      end
    end

    def verify?(variant)
      ENV["FRACTAL_VERIFY"] == "1" && variant.replayable
    end

    def wrap(target, variant, original, installed)
      key = variant.key
      target.define_method(variant.method) do |*args, **kwargs, &block|
        # A block could observe being called twice.
        next installed.bind_call(self, *args, **kwargs, &block) if block

        Fractal.compare(
          key,
          -> { original.bind_call(self, *args, **kwargs) },
          -> { installed.bind_call(self, *args, **kwargs) }
        )
      end
    end

    def outcome
      [:ok, yield]
    rescue StandardError => e
      [:raise, e]
    end

    def same?(expected, actual)
      return false unless expected[0] == actual[0]
      return expected[1] == actual[1] if expected[0] == :ok

      expected[1].class == actual[1].class && expected[1].message == actual[1].message
    end

    def describe(outcome)
      outcome[0] == :ok ? outcome[1].inspect : "#{outcome[1].class}: #{outcome[1].message}"
    end
  end
end
