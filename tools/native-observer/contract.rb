# frozen_string_literal: true
require "json"
require "open3"
require "rbconfig"

# Fresh VMs execute the identical fixture; no exporter, mocks or compiler.
if ARGV.empty?
  outputs = {}
  %w[baseline inactive active].each do |mode|
    out, err, status = Open3.capture3(RbConfig.ruby, __FILE__, mode)
    abort "#{mode} failed:\n#{out}#{err}" unless status.success?
    outputs[mode] = JSON.parse(out)
    puts "#{mode}: #{outputs[mode].fetch('observations').size} native assertions; #{outputs[mode].fetch('events')} notifications"
  end
  reference = outputs.fetch("baseline").fetch("observations")
  %w[inactive active].each do |mode|
    abort "#{mode} differs from original MRI" unless outputs.fetch(mode).fetch("observations") == reference
  end
  out, err, status = Open3.capture3(RbConfig.ruby, __FILE__, "ruby-control")
  abort "comparison failed to detect the Ruby-frame visibility defect" if status.success? || !err.include?("private callback")
  puts "Ruby-wrapper negative control: detected private callback becoming public"
  puts err.lines.first.strip
  out, err, status = Open3.capture3(RbConfig.ruby, '-rnative_define_method_observer', '-e', <<~RUBY)
    NativeDefineMethodObserver.collector = ->(*) { Process.kill('TERM', Process.pid); sleep 0.1 }
    Class.new.define_method(:probe) { 7 }
    abort 'TERM swallowed'
  RUBY
  abort "collector swallowed termination: #{out}#{err}" unless status.termsig == Signal.list.fetch('TERM')
  %w[interrupt exit throw].each do |unwind|
    out, err, status = Open3.capture3(RbConfig.ruby, __FILE__, "unwind-#{unwind}")
    abort "#{unwind} control failed: #{out}#{err}" unless status.success?
  end
  puts 'PASS: collector preserves TERM, Interrupt, SystemExit and throw'
  puts "PASS: original == native-inactive == native-active"
  puts "runtime: #{outputs.fetch('baseline').fetch('runtime')}"
  puts JSON.pretty_generate(outputs)
  exit
end

mode = ARGV.fetch(0)
if mode.start_with?('unwind-')
  require 'native_define_method_observer'
  klass = Class.new
  case mode
  when 'unwind-throw'
    NativeDefineMethodObserver.collector = ->(*) { throw :finished, -29 }
    result = catch(:finished) { klass.define_method(:probe) { 7 }; 101 }
    abort 'throw swallowed' unless result == -29
  when 'unwind-interrupt', 'unwind-exit'
    expected = mode == 'unwind-interrupt' ? Interrupt.new : SystemExit.new(23)
    NativeDefineMethodObserver.collector = ->(*) { raise expected }
    caught = nil
    begin
      klass.define_method(:probe) { 7 }
    rescue Exception => error
      caught = error
    end
    abort 'unwind identity lost' unless caught.equal?(expected)
  end
  abort 'installed body changed' unless klass.new.probe == 7
  abort 'unwind deferred as ordinary failure' unless NativeDefineMethodObserver.failures.empty?
  exit
end
events = []
if %w[inactive active].include?(mode)
  require "native_define_method_observer"
  if mode == "active"
    NativeDefineMethodObserver.collector = ->(owner, name, callable) { events << [owner, name, callable] }
  end
elsif mode == "ruby-control"
  Module.prepend(Module.new do
    def define_method(*args, &block)
      super
    end
  end)
elsif mode != "baseline"
  abort "unknown mode: #{mode}"
end

observations = {}
check = lambda do |label, expected, actual|
  raise "#{label}: expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
  observations[label] = actual
end
visibility = lambda do |owner, name|
  %i[private protected public].find { |kind| owner.public_send("#{kind}_instance_methods", false).include?(name) }
end

%i[private protected public].each do |kind|
  callbacks = []
  mod = Module.new do
    define_singleton_method(:method_added) do |name|
      callbacks << [name, visibility.call(self, name), Object.new.extend(self).send(name)]
    end
    send(kind)
    define_method(:hidden) { 7 }
  end
  check.call("#{kind} callback", [[:hidden, kind, 7]], callbacks)
  check.call("#{kind} final", kind, visibility.call(mod, :hidden))
end

# This asymmetric explicit Proc must win over the ignored block.
offset = 19
explicit = proc { |n| n * 3 + offset }
klass = Class.new
ignored = 0
check.call("symbol return", :chosen, klass.define_method(:chosen, explicit) { |n| ignored += 1; n * 101 - 43 })
instance = klass.new
check.call("explicit Proc negative", -2, instance.chosen(-7))
offset = -11
check.call("explicit Proc rebinding", 4, instance.chosen(5))
check.call("ignored block not run", 0, ignored)
check.call("explicit Proc arity", 1, instance.method(:chosen).arity)
begin
  instance.chosen
rescue ArgumentError => error
  check.call("Proc strict arity", "wrong number of arguments (given 0, expected 1)", error.message)
end
check.call("alias return", :aliased, klass.alias_method(:aliased, :chosen))
check.call("alias equality", true, klass.instance_method(:aliased) == klass.instance_method(:chosen))
check.call("alias original name", :chosen, klass.instance_method(:aliased).original_name)

method = instance.method(:aliased)
copy = Class.new(klass)
check.call("Method copy return", :bound_copy, copy.define_method(:bound_copy, method) { -999 })
check.call("Method copy result", -17, copy.new.bound_copy(-2))
unbound = klass.instance_method(:aliased)
check.call("UnboundMethod copy return", :unbound_copy, copy.define_method(:unbound_copy, unbound))
check.call("UnboundMethod copy result", 10, copy.new.unbound_copy(7))
check.call("copy original name", :chosen, copy.instance_method(:unbound_copy).original_name)

calls = 0
name = Object.new
name.define_singleton_method(:to_str) { calls += 1; "coerced" }
check.call("coerced return", :coerced, klass.define_method(name) { |a, k:, &b| b.call(a * 2 + k) })
check.call("name coerced once", 1, calls)
check.call("keywords and block", 24, instance.coerced(-4, k: 20) { |n| n * 2 })
check.call("literal block arity", 2, instance.method(:coerced).arity)
check.call("literal block parameters", [[:req, :a], [:keyreq, :k], [:block, :b]], instance.method(:coerced).parameters)
check.call("installed literal is lambda", true, instance.method(:coerced).to_proc.lambda?)
klass.define_method(:literal_pair) { |a, b| a * 7 - b }
check.call("literal pair asymmetric", -31, instance.literal_pair(-4, 3))
begin
  instance.literal_pair([-4, 3])
rescue ArgumentError => error
  check.call("literal array is not destructured", "wrong number of arguments (given 1, expected 2)", error.message)
end
klass.define_method(:symbol_proc, &:upcase)
check.call("Symbol Proc forwarding", "AB", instance.symbol_proc("ab"))

def returning_class
  Class.new do
    define_method(:local_return) { return 47 }
    define_method(:local_break) { break -53 }
  end
end
returning = returning_class.new
check.call("literal return stays local", 47, returning.local_return)
check.call("literal break stays local", -53, returning.local_break)
block_proc = proc { |a, b| a * 7 - b }
klass.define_method(:passed_proc, &block_proc)
check.call("passed Proc asymmetric", -31, instance.passed_proc(-4, 3))
check.call("passed Proc strict parameters", [[:req, :a], [:req, :b]], instance.method(:passed_proc).parameters)

callbacks = []
functions = Module.new do
  define_singleton_method(:method_added) do |name|
    callbacks << [:instance, name, visibility.call(self, name), Object.new.extend(self).send(name)]
  end
  define_singleton_method(:singleton_method_added) do |name|
    callbacks << [:singleton, name, singleton_class.public_method_defined?(name), public_send(name)] if name == :mf
  end
  module_function
  define_method(:mf) { |n = -3| n * 5 + 2 }
end
check.call("module_function callbacks", [[:instance, :mf, :private, -13], [:singleton, :mf, true, -13]], callbacks)
check.call("module_function singleton", 37, functions.mf(7))
check.call("module_function instance visibility", :private, visibility.call(functions, :mf))

module NativeObserverLexicalControl
  module_function
  define_method(:local_return) { return 47 }
  define_method(:local_break) { break -53 }
end
check.call("module_function literal return", 47, NativeObserverLexicalControl.local_return)
check.call("module_function literal break", -53, NativeObserverLexicalControl.local_break)
check.call("module_function literal instance", 47, Object.new.extend(NativeObserverLexicalControl).send(:local_return))

callbacks = []
first = proc { 13 }
second = proc { -29 }
reentrant = Module.new do
  replacing = false
  define_singleton_method(:method_added) do |name|
    callbacks << [name, visibility.call(self, name), Object.new.extend(self).send(name)]
    unless replacing
      replacing = true
      define_method(name, second)
    end
  end
  private
  define_method(:replaced, first)
end
check.call("reentrant callbacks", [[:replaced, :private, 13], [:replaced, :private, -29]], callbacks)
check.call("reentrant final body", -29, Object.new.extend(reentrant).send(:replaced))
check.call("reentrant final visibility", :private, visibility.call(reentrant, :replaced))

# A successful native return does not guarantee that the method still exists.
removed = Module.new do
  define_singleton_method(:method_added) { |name| remove_method(name) }
end
check.call("callback removal return", :gone, removed.define_method(:gone) { 61 })
check.call("callback removal retained", false, removed.method_defined?(:gone))

coercion_error = RuntimeError.new("application name coercion exception")
bad_name = Object.new
bad_name.define_singleton_method(:to_str) { raise coercion_error }
before_errors = events.size
[
  ["no args", ArgumentError, -> { klass.define_method }],
  ["three args", ArgumentError, -> { klass.define_method(:bad, explicit, explicit) }],
  ["no body", ArgumentError, -> { klass.define_method(:bad) }],
  ["bad body with valid block", TypeError, -> { klass.define_method(:bad, 42) { 5 } }],
  ["keyword body", TypeError, -> { klass.define_method(:bad, body: explicit) }],
  ["bad name", TypeError, -> { klass.define_method(nil, explicit) }],
  ["incompatible Method", TypeError, -> { Class.new.define_method(:bad, method) }],
  ["frozen receiver", FrozenError, -> { Module.new.freeze.define_method(:bad, explicit) }]
].each do |label, type, operation|
  caught = nil
  begin
    operation.call
  rescue Exception => error
    caught = error
  end
  check.call(label, type.name, caught&.class&.name)
end
begin
  klass.define_method(bad_name) { 17 }
rescue RuntimeError => error
  check.call("coercion exception identity", true, error.equal?(coercion_error))
end
callback_error = RuntimeError.new("application callback exception")
failing = Module.new do
  define_singleton_method(:method_added) { |_| raise callback_error }
end
begin
  failing.define_method(:installed_before_raise) { 31 }
rescue RuntimeError => error
  check.call("callback exception identity", true, error.equal?(callback_error))
end
check.call("callback exception leaves installation", 31, Object.new.extend(failing).installed_before_raise)
check.call("failed calls not notified", true, before_errors == events.size)

if mode == "active"
  # Object identity is captured before native callbacks, not reconstructed later.
  raise "successful calls not notified exactly once" unless events.size == 18
  raise "explicit body provenance lost" unless events.find { |o, n, _| o == klass && n == :chosen }.last.equal?(explicit)
  raise "Method provenance lost" unless events.find { |o, n, _| o == copy && n == :bound_copy }.last.equal?(method)
  raise "UnboundMethod provenance lost" unless events.find { |o, n, _| o == copy && n == :unbound_copy }.last.equal?(unbound)
  bodies = events.select { |o, _, _| o == reentrant }.map(&:last)
  raise "reentrant provenance/order lost" unless bodies == [second, first]
  block = events.find { |o, n, _| o == klass && n == :coerced }.last
  raise "literal block provenance lost" unless block.is_a?(Proc) && block.source_location.first == __FILE__
  raise "passed Proc identity lost" unless events.find { |o, n, _| o == klass && n == :passed_proc }.last.equal?(block_proc)
  raise "closure binding lost" unless explicit.binding.local_variable_get(:offset) == -11

  # GC in the native method_added callback must not lose the saved Proc, and
  # collector notification must happen after that application's callback.
  order = []
  compacted = Class.new do
    define_singleton_method(:method_added) do |_|
      GC.verify_compaction_references(double_heap: true, toward: :empty)
      order << :method_added
    end
  end
  captured = nil
  NativeDefineMethodObserver.collector = ->(_, _, body) { order << :collector; captured = body }
  compacted.define_method(:gc_body) { 79 }
  raise "notification ran before application callback" unless order == [:method_added, :collector]
  raise "Proc lost across compaction" unless captured.is_a?(Proc) && compacted.new.gc_body == 79

  # Exporter limitations must not change a successful application's return/$!.
  unsupported = RuntimeError.new("exporter limitation")
  NativeDefineMethodObserver.collector = ->(*) { raise unsupported }
  original = RuntimeError.new("already handling application error")
  begin
    raise original
  rescue RuntimeError
    raise "return changed by collector failure" unless klass.define_method(:collector_failure) { 41 } == :collector_failure
    raise "application $! lost" unless $!.equal?(original)
  end
  failure = NativeDefineMethodObserver.failures.last
  raise "collector failure not retained" unless failure == [klass, :collector_failure, unsupported]
  raise "successful body lost" unless instance.collector_failure == 41
  NativeDefineMethodObserver.collector = nil
  puts JSON.generate(observer_checks: {
    successful_notifications: events.size,
    explicit_proc_method_unbound_and_passed_proc_identity: true,
    literal_source_and_closure_binding: true,
    captured_literal_proc_lambda: block.lambda?,
    installed_literal_method_proc_lambda: instance.method(:coerced).to_proc.lambda?,
    reentrant_notification_bodies: [second.call, first.call],
    callback_then_collector: order,
    compaction_keeps_callable: true,
    collector_failure_keeps_return_body_and_application_error: true
  }, runtime: { ruby: RUBY_DESCRIPTION, yjit: RubyVM::YJIT.enabled? }, observations: observations, events: events.size)
else
  puts JSON.generate(runtime: { ruby: RUBY_DESCRIPTION, yjit: RubyVM::YJIT.enabled? }, observations: observations, events: events.size)
end
