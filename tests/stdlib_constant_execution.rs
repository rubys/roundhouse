//! Ruby and Spinel supply these constants; resolving source names must
//! preserve the constructors and rescue classes in emitted Ruby.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn struct_mutex_and_json_error_constants_execute_after_emission() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/registry.rb", r#"require "json"
class Registry
  Entry = Struct.new(:lock, :members)
  def self.entry
    Entry.new(Mutex.new, {})
  end
  def self.parse(text)
    JSON.parse(text)
  rescue JSON::ParserError
    nil
  end
end
"#)
        .run_ruby(r#"
entry = Registry.entry
entry.lock.synchronize { entry.members["member"] = 7 }
raise "struct or mutex lost" unless entry.members == { "member" => 7 }
raise "valid JSON changed" unless Registry.parse('{"ok":true}') == { "ok" => true }
raise "JSON rescue lost" unless Registry.parse('{') == nil
puts "stdlib constant execution passed"
"#)
        .assert_passes();
}

fn stdlib_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

const QUEUE_SOURCE: &str = r#"class QueueProbe
  def self.queue_class
    Queue
  end
  def self.build
    Queue.new
  end
  def self.bounded_queue_class
    SizedQueue
  end
  def self.build_bounded
    SizedQueue.new(2)
  end
  def self.thread_classes
    [Thread::Queue, Thread::SizedQueue, Thread::Mutex]
  end
  def self.build_thread_queue
    Thread::Queue.new
  end
  def self.build_thread_bounded_queue
    Thread::SizedQueue.new(2)
  end
  def self.build_thread_mutex
    Thread::Mutex.new
  end
end
"#;

const QUEUE_SCRIPT: &str = r#"
raise "Queue constant lost" unless QueueProbe.queue_class == Queue
queue = QueueProbe.build
queue.push(17)
queue.push(29)
raise "Queue order changed" unless queue.pop == 17 && queue.pop == 29 && queue.empty?
puts "Queue execution passed"
"#;

#[test]
fn queue_constants_execute_after_emission() {
    stdlib_app().write("app/services/queue_probe.rb", QUEUE_SOURCE)
        .run_ruby(QUEUE_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn queue_constants_execute_in_native_spinel() {
    stdlib_app().write("app/services/queue_probe.rb", QUEUE_SOURCE)
        .run_spinel(QUEUE_SCRIPT).assert_passes();
}

const SIZED_QUEUE_SCRIPT: &str = r#"
raise "SizedQueue constant lost" unless QueueProbe.bounded_queue_class == SizedQueue
queue = QueueProbe.build_bounded
queue.push(17)
queue.push(29)
raise "SizedQueue capacity changed" unless queue.max == 2 && queue.size == 2
raise "SizedQueue order changed" unless queue.pop == 17 && queue.pop == 29 && queue.empty?
puts "SizedQueue execution passed"
"#;

#[test]
fn sized_queue_constants_execute_after_emission() {
    stdlib_app().write("app/services/queue_probe.rb", QUEUE_SOURCE)
        .run_ruby(SIZED_QUEUE_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn sized_queue_constants_execute_in_native_spinel() {
    stdlib_app().write("app/services/queue_probe.rb", QUEUE_SOURCE)
        .run_spinel(SIZED_QUEUE_SCRIPT).assert_passes();
}

const THREAD_ALIASES_SCRIPT: &str = r#"
raise "Thread aliases lost" unless QueueProbe.thread_classes == [Queue, SizedQueue, Mutex]
queue = QueueProbe.build_thread_queue
queue.push(41)
raise "Thread::Queue changed" unless queue.pop == 41
bounded = QueueProbe.build_thread_bounded_queue
bounded.push(53)
raise "Thread::SizedQueue changed" unless bounded.max == 2 && bounded.pop == 53
mutex = QueueProbe.build_thread_mutex
raise "Thread::Mutex changed" unless mutex.synchronize { 67 } == 67
puts "Thread aliases execution passed"
"#;

#[test]
fn thread_queue_and_mutex_alias_constants_execute_after_emission() {
    stdlib_app().write("app/services/queue_probe.rb", QUEUE_SOURCE)
        .run_ruby(THREAD_ALIASES_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn thread_queue_and_mutex_alias_constants_execute_in_native_spinel() {
    stdlib_app().write("app/services/queue_probe.rb", QUEUE_SOURCE)
        .run_spinel(THREAD_ALIASES_SCRIPT).assert_passes();
}

const QUEUE_ERRORS_SOURCE: &str = r#"class QueueErrorProbe
  def self.error_classes
    [ThreadError, ClosedQueueError]
  end
  def self.pop_empty(queue)
    queue.pop(true)
  rescue ThreadError
    "empty"
  end
  def self.push_closed(queue)
    queue.push(37)
  rescue ClosedQueueError
    "closed"
  end
end
"#;

const QUEUE_ERRORS_SCRIPT: &str = r#"
raise "Queue error constants lost" unless QueueErrorProbe.error_classes == [ThreadError, ClosedQueueError]
raise "ThreadError hierarchy changed" unless QueueErrorProbe.error_classes[0].superclass == StandardError
raise "ClosedQueueError hierarchy changed" unless QueueErrorProbe.error_classes[1].superclass == StopIteration
queue = Queue.new
raise "ThreadError rescue lost" unless QueueErrorProbe.pop_empty(queue) == "empty"
queue.close
raise "ClosedQueueError rescue lost" unless QueueErrorProbe.push_closed(queue) == "closed"
puts "Queue error execution passed"
"#;

#[test]
fn queue_error_constants_execute_after_emission() {
    stdlib_app().write("app/services/queue_error_probe.rb", QUEUE_ERRORS_SOURCE)
        .run_ruby(QUEUE_ERRORS_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn queue_error_constants_execute_in_native_spinel() {
    stdlib_app().write("app/services/queue_error_probe.rb", QUEUE_ERRORS_SOURCE)
        .run_spinel(QUEUE_ERRORS_SCRIPT).assert_passes();
}

const GENERATOR_ERROR_SOURCE: &str = r#"require "json"
class GeneratorErrorProbe
  def self.generate(value)
    JSON.generate(value)
  rescue JSON::GeneratorError
    "generation failed"
  end
  def self.generate_rooted(value)
    JSON.generate(value)
  rescue ::JSON::GeneratorError
    "generation failed"
  end
end
"#;

const GENERATOR_ERROR_SCRIPT: &str = r#"
raise "valid JSON changed" unless GeneratorErrorProbe.generate({ "ok" => true }) == '{"ok":true}'
raise "JSON generation rescue lost" unless GeneratorErrorProbe.generate(Float::NAN) == "generation failed"
raise "rooted valid JSON changed" unless GeneratorErrorProbe.generate_rooted({ "ok" => true }) == '{"ok":true}'
raise "rooted JSON generation rescue lost" unless GeneratorErrorProbe.generate_rooted(Float::NAN) == "generation failed"
puts "JSON::GeneratorError execution passed"
"#;

#[test]
fn json_generator_error_constants_execute_after_emission() {
    stdlib_app().write("app/services/generator_error_probe.rb", GENERATOR_ERROR_SOURCE)
        .write("app/services/generator_error_class_probe.rb", "class GeneratorErrorClassProbe\n  def self.error_class\n    JSON::GeneratorError\n  end\nend\n")
        .run_ruby(&format!(r#"
raise "GeneratorError constant lost" unless GeneratorErrorClassProbe.error_class == JSON::GeneratorError
raise "GeneratorError hierarchy changed" unless GeneratorErrorClassProbe.error_class.superclass == JSON::JSONError
{GENERATOR_ERROR_SCRIPT}
"#))
        .assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn json_generator_error_constants_execute_in_native_spinel() {
    stdlib_app().write("app/services/generator_error_probe.rb", GENERATOR_ERROR_SOURCE)
        .run_spinel(GENERATOR_ERROR_SCRIPT).assert_passes();
}

const COMPARABLE_SOURCE: &str = r#"class ComparableProbe
  include Comparable
  def initialize(value)
    @value = value
  end
  def value
    @value
  end
  def <=>(other)
    value <=> other.value
  end
  def self.comparison_module
    Comparable
  end
end
"#;

const COMPARABLE_SCRIPT: &str = r#"
raise "Comparable constant lost" unless ComparableProbe.comparison_module == Comparable
raise "Comparable is not a module" unless ComparableProbe.comparison_module.class == Module
low = ComparableProbe.new(3)
equal = ComparableProbe.new(3)
high = ComparableProbe.new(8)
raise "Comparable mixin lost" unless low < high && high > low && low == equal && low <= equal
puts "Comparable execution passed"
"#;

#[test]
fn comparable_constants_execute_after_emission() {
    stdlib_app().write("app/services/comparable_probe.rb", COMPARABLE_SOURCE)
        .run_ruby(COMPARABLE_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn comparable_constants_execute_in_native_spinel() {
    stdlib_app().write("app/services/comparable_probe.rb", COMPARABLE_SOURCE)
        .run_spinel(COMPARABLE_SCRIPT).assert_passes();
}

const ENUMERABLE_SOURCE: &str = r#"class EnumerableProbe
  include Enumerable
  def each
    yield 2
    yield 5
    self
  end
  def self.enumeration_module
    Enumerable
  end
end
"#;

const ENUMERABLE_SCRIPT: &str = r#"
raise "Enumerable constant lost" unless EnumerableProbe.enumeration_module == Enumerable
raise "Enumerable is not a module" unless EnumerableProbe.enumeration_module.class == Module
values = EnumerableProbe.new.map { |value| value * 3 }
raise "Enumerable mixin lost" unless values == [6, 15]
puts "Enumerable execution passed"
"#;

#[test]
fn enumerable_constants_execute_after_emission() {
    stdlib_app().write("app/services/enumerable_probe.rb", ENUMERABLE_SOURCE)
        .run_ruby(ENUMERABLE_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn enumerable_constants_execute_in_native_spinel() {
    stdlib_app().write("app/services/enumerable_probe.rb", ENUMERABLE_SOURCE)
        .run_spinel(ENUMERABLE_SCRIPT).assert_passes();
}

const SYNCHRONIZATION_SOURCE: &str = r#"require "monitor"
class SynchronizationProbe
  def self.reentrant_count
    lock = Monitor.new
    count = 0
    lock.synchronize { lock.synchronize { count += 1 } }
    count
  end
  def self.handoff
    mutex = Mutex.new
    ready = ConditionVariable.new
    value = 0
    thread = Thread.new do
      mutex.synchronize do
        value = 42
        ready.signal
      end
    end
    mutex.synchronize { ready.wait(mutex) while value == 0 }
    thread.join
    value
  end
  def self.condition_variable_classes
    [ConditionVariable, Thread::ConditionVariable]
  end
end
"#;

const SYNCHRONIZATION_SCRIPT: &str = r#"
raise "Monitor reentrancy lost" unless SynchronizationProbe.reentrant_count == 1
raise "ConditionVariable handoff lost" unless SynchronizationProbe.handoff == 42
raise "ConditionVariable alias lost" unless SynchronizationProbe.condition_variable_classes.uniq.length == 1
puts "Monitor and ConditionVariable execution passed"
"#;

#[test]
fn monitor_and_condition_variable_constants_execute_after_emission() {
    stdlib_app().write("app/models/synchronization_probe.rb", SYNCHRONIZATION_SOURCE)
        .run_ruby(SYNCHRONIZATION_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn monitor_and_condition_variable_constants_execute_in_native_spinel() {
    stdlib_app().write("app/models/synchronization_probe.rb", SYNCHRONIZATION_SOURCE)
        .run_spinel(SYNCHRONIZATION_SCRIPT).assert_passes();
}

const UNIX_SOCKET_SOURCE: &str = r#"require "socket"
class UnixSocketProbe
  def self.round_trip(path)
    server = UNIXServer.new(path)
    client = UNIXSocket.new(path)
    peer = server.accept
    client.write("ping\n")
    line = peer.readpartial(5)
    client.close
    peer.close
    server.close
    line
  end
  def self.unix_socket?(value)
    value.is_a?(UNIXSocket)
  end
end
"#;

const UNIX_SOCKET_SCRIPT: &str = r#"
require "tmpdir"
Dir.mktmpdir do |dir|
  raise "UNIX socket round trip lost" unless UnixSocketProbe.round_trip(File.join(dir, "probe.sock")) == "ping\n"
end
raise "UNIXSocket class check changed" if UnixSocketProbe.unix_socket?("probe.sock")
puts "UNIXServer and UNIXSocket execution passed"
"#;

#[test]
fn unix_socket_constants_execute_after_emission() {
    stdlib_app().write("app/models/unix_socket_probe.rb", UNIX_SOCKET_SOURCE)
        .run_ruby(UNIX_SOCKET_SCRIPT).assert_passes();
}

/// The two socket classes name one library, and the emitted file requires it once.
#[test]
fn unix_socket_classes_share_one_require() {
    let (emitted, errors) = stdlib_app().write("app/models/unix_socket_probe.rb", UNIX_SOCKET_SOURCE)
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let probe = std::fs::read_to_string(emitted.join("app/models/unix_socket_probe.rb")).expect("emitted probe");
    assert_eq!(probe.matches("require \"socket\"").count(), 1, "{probe}");
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn unix_socket_constants_execute_in_native_spinel() {
    stdlib_app().write("app/models/unix_socket_probe.rb", UNIX_SOCKET_SOURCE)
        .run_spinel(UNIX_SOCKET_SCRIPT).assert_passes();
}

const OPEN3_AND_FIND_SOURCE: &str = r#"require "open3"
require "find"
class ProcessAndTreeProbe
  def self.echo(text)
    stdout, _stderr, status = Open3.capture3("cat", stdin_data: text)
    status.success? ? stdout : ""
  end
  def self.files(root)
    found = []
    Find.find(root) do |path|
      Find.prune if File.basename(path) == "skip"
      found << path if File.file?(path)
    end
    found.sort
  end
end
"#;

const OPEN3_AND_FIND_SCRIPT: &str = r#"
require "tmpdir"
raise "Open3.capture3 lost" unless ProcessAndTreeProbe.echo("hi") == "hi"
Dir.mktmpdir do |dir|
  Dir.mkdir(File.join(dir, "skip"))
  File.write(File.join(dir, "skip", "hidden.txt"), "x")
  File.write(File.join(dir, "kept.txt"), "x")
  raise "Find.find or prune lost" unless ProcessAndTreeProbe.files(dir) == [File.join(dir, "kept.txt")]
end
puts "Open3 and Find execution passed"
"#;

#[test]
fn open3_and_find_constants_execute_after_emission() {
    stdlib_app().write("app/models/process_and_tree_probe.rb", OPEN3_AND_FIND_SOURCE)
        .run_ruby(OPEN3_AND_FIND_SCRIPT).assert_passes();
}

/// Without a block `Find.find` answers an Enumerator, not nil.
#[test]
fn find_without_a_block_is_an_enumerator() {
    stdlib_app()
        .write("app/models/item.rb", "require \"find\"\nclass Item < ActiveRecord::Base\n  def self.files(root)\n    Find.find(root).select { |path| File.file?(path) }.length\n  end\nend\n")
        .run_ruby("require \"tmpdir\"\nDir.mktmpdir do |dir|\n  File.write(File.join(dir, \"a.txt\"), \"x\")\n  raise \"Find.find enumerator lost\" unless Item.files(dir) == 1\nend\nputs \"Find enumerator passed\"\n")
        .assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn open3_and_find_constants_execute_in_native_spinel() {
    stdlib_app().write("app/models/process_and_tree_probe.rb", OPEN3_AND_FIND_SOURCE)
        .run_spinel(OPEN3_AND_FIND_SCRIPT).assert_passes();
}
