//! Class-body @ivar assignments initialize the class object at load time.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/counters\", to: \"counters#index\"\nend\n")
        .write("app/services/counter.rb", r#"class Counter
  @value = 1
  FIRST = @value
  @value = 2
  SECOND = @value
  def self.value
    @value
  end
  def self.bump
    @value += 1
    @value
  end
end
"#)
        .write("app/services/other_counter.rb", r#"class OtherCounter
  @value = 8
  def self.value
    @value
  end
end
"#)
        .write("app/controllers/counters_controller.rb", r#"class CountersController < ApplicationController
  def index
    render plain: Counter.bump.to_s + "," + OtherCounter.value.to_s
  end
end
"#)
}

const SOURCE_ORDER: &str = r#"
raise "first initializer lost" unless Counter::FIRST == 1
raise "second initializer lost" unless Counter::SECOND == 2
raise "class state lost" unless Counter.value == 2
raise "other class state lost" unless OtherCounter.value == 8
require_relative "app/controllers/counters_controller"
controller = CountersController.new
controller.process_action(:index)
raise "class write lost" unless controller.body == "3,8"
require_relative "app/models/counter"
raise "initializer ran twice" unless Counter.value == 3
raise "classes share storage" unless OtherCounter.value == 8
puts "Class instance state contract passed"
"#;

#[test]
fn class_instance_state_initializes_in_source_order() {
    app().run_ruby(SOURCE_ORDER).assert_passes();
}

fn require_split_app() -> emit_and_run::Overlay {
    app()
        .write(
            "app/services/counter.rb",
            r#"class Counter
  @value = 1
  FIRST = 1
  @value = CounterReader::VALUE
  SECOND = @value
  NEXT = 2
  @value = LastReader::VALUE
  @value = 3
  THIRD = @value
  def self.request_only
    RequestReader::VALUE
  end
  def self.value
    @value
  end
  def self.bump
    @value += 1
    @value
  end
end
"#,
        )
        .write(
            "app/services/counter_reader.rb",
            "class CounterReader\n  @first = Counter::FIRST\n  FIRST = @first\n  VALUE = 1\nend\n",
        )
        .write(
            "app/services/last_reader.rb",
            "class LastReader\n  @snapshot = Counter::NEXT\n  SNAPSHOT = @snapshot\n  VALUE = 2\nend\n",
        )
        .write(
            "app/services/request_reader.rb",
            "class RequestReader\n  @snapshot = Counter::FIRST\n  VALUE = 3\nend\n",
        )
}

const REQUIRE_SPLIT: &str = r#"
raise "first initializer lost" unless Counter::FIRST == 1
raise "required file saw uninitialized state" unless Counter::SECOND == 1
raise "reader initializer ran too early" unless CounterReader::FIRST == 1
raise "second dependency ran early" unless LastReader::SNAPSHOT == 2
raise "tail initializer lost" unless Counter::THIRD == 3
raise "state reordered across require" unless Counter.value == 3
raise "request-only dependency loaded too early" unless Counter.request_only == 3
puts "Class instance state require contract passed"
"#;

#[test]
fn class_instance_state_keeps_its_order_across_a_require_split() {
    require_split_app().run_ruby(REQUIRE_SPLIT).assert_passes();
}

fn module_state_app() -> emit_and_run::Overlay {
    app()
        .edit("app/services/counter.rb", "class Counter", "module Counter")
}

const MODULE_STATE: &str = r#"
raise "module initializer lost" unless Counter::FIRST == 1 && Counter::SECOND == 2
raise "module state lost" unless Counter.value == 2
raise "module state is not writable" unless Counter.bump == 3
raise "module and class share storage" unless OtherCounter.value == 8
puts "Module instance state contract passed"
"#;

#[test]
fn module_instance_state_initializes_the_module_object() {
    module_state_app().run_ruby(MODULE_STATE).assert_passes();
}

fn method_order_app() -> emit_and_run::Overlay {
    app()
        .write("app/services/method_counter.rb", r#"class MethodCounter
  @value = 1
  def self.read
    @value
  end
  FIRST = read
  @value = FIRST + 1
  SECOND = read
  @value = self.read + 3
end
"#)
}

const METHOD_ORDER: &str = r#"raise "first snapshot" unless MethodCounter::FIRST == 1
raise "second snapshot" unless MethodCounter::SECOND == 2
raise "method initializer" unless MethodCounter.read == 5
puts "method order passed"
"#;

#[test]
fn class_instance_state_preserves_method_order() {
    method_order_app().run_ruby(METHOD_ORDER).assert_passes();
}

const REDEFINED_SOURCE: &str = r#"class MethodCounter
  @value = 1
  def self.value
    @value + 10
  end
  FIRST = value
  @value = FIRST + 1
  class << self
    attr_accessor :value
  end
  SECOND = value
  @value = value + 3
end
"#;

fn redefined_accessor_app() -> emit_and_run::Overlay {
    app()
        .write("app/services/method_counter.rb", REDEFINED_SOURCE)
}

const REDEFINED_ACCESSOR: &str = r#"raise "first snapshot" unless MethodCounter::FIRST == 11
raise "second snapshot" unless MethodCounter::SECOND == 12
raise "method initializer" unless MethodCounter.value == 15
puts "method order passed"
"#;

#[test]
fn class_instance_state_orders_redefined_accessors() {
    redefined_accessor_app().run_ruby(REDEFINED_ACCESSOR).assert_passes();
}

fn subclass_accessors_app() -> emit_and_run::Overlay {
    app()
        .write("app/services/child_counter.rb", r#"class ChildCounter < ParentCounter
end
"#)
        .write("app/services/other_counter.rb", r#"class OtherCounter < ParentCounter
  @value = 13
end
"#)
        .write("app/services/parent_counter.rb", r#"class ParentCounter
  @value = 7
  class << self
    attr_accessor :value
  end
end
"#)
}

const SUBCLASS_ACCESSORS: &str = r#"raise "parent missing" unless ParentCounter.value == 7
raise "subclass inherited state" unless ChildCounter.value.nil?
raise "subclass lost own state" unless OtherCounter.value == 13
ChildCounter.value = 29
raise "subclass write lost" unless ChildCounter.value == 29
raise "subclass overwrote parent" unless ParentCounter.value == 7
raise "siblings share state" unless OtherCounter.value == 13
puts "subclass accessors passed"
"#;

#[test]
fn class_instance_state_preserves_subclass_accessors() {
    subclass_accessors_app().run_ruby(SUBCLASS_ACCESSORS).assert_passes();
}

const COMPOUND_SOURCE: &str = r#"class DefaultCounter
  @value ||= 7
  FIRST = @value
  @value ||= 1 / 0
  @falsey = false
  @falsey ||= 13
  FALSEY = @falsey
  @unset &&= 1 / 0
  UNSET = @unset
  @value += 5
  SECOND = @value
  @value &&= 14
  def self.value
    @value
  end
end
"#;

fn compound_state_app(is_module: bool) -> emit_and_run::Overlay {
    let source = if is_module {
        COMPOUND_SOURCE.replacen("class DefaultCounter", "module DefaultCounter", 1)
    } else {
        COMPOUND_SOURCE.to_string()
    };
    app().write("app/services/default_counter.rb", &source)
}

const COMPOUND_STATE: &str = r#"raise "default initializer lost" unless DefaultCounter::FIRST == 7
raise "falsey default lost" unless DefaultCounter::FALSEY == 13
raise "unset &&= ran" unless DefaultCounter::UNSET.nil?
raise "compound write reordered" unless DefaultCounter::SECOND == 12
raise "truthy &&= lost" unless DefaultCounter.value == 14
require_relative "app/models/default_counter"
raise "compound initializer ran twice" unless DefaultCounter.value == 14
puts "compound class state passed"
"#;

#[test]
fn class_instance_state_retains_compound_writes() {
    compound_state_app(false).run_ruby(COMPOUND_STATE).assert_passes();
}

#[test]
fn module_instance_state_retains_compound_writes() {
    compound_state_app(true).run_ruby(COMPOUND_STATE).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn class_instance_state_runs_natively() {
    app().run_spinel(SOURCE_ORDER).assert_passes();
    require_split_app().run_spinel(REQUIRE_SPLIT).assert_passes();
    module_state_app().run_spinel(MODULE_STATE).assert_passes();
    method_order_app().run_spinel(METHOD_ORDER).assert_passes();
    subclass_accessors_app().run_spinel(SUBCLASS_ACCESSORS).assert_passes();
    compound_state_app(false).run_spinel(COMPOUND_STATE).assert_passes();
    compound_state_app(true).run_spinel(COMPOUND_STATE).assert_passes();
}

#[test]
fn spinel_refuses_class_state_with_redefined_methods() {
    let mut app = roundhouse::App::default();
    app.library_classes.push(
        roundhouse::ingest::ingest_library_class(REDEFINED_SOURCE.as_bytes(), "counter.rb")
            .unwrap().unwrap(),
    );
    let error = roundhouse::project::target_files(
        &app, std::path::Path::new("."), roundhouse::project::BuildTarget::Spinel,
    ).unwrap_err();
    assert!(error.contains("method redefinition is not supported (spinel)"), "{error}");
}

#[test]
fn spinel_refuses_class_state_with_redefined_methods_across_reopens() {
    for receiver in ["self.", ""] {
        for initializer_first in [true, false] {
            let call = if receiver.is_empty() { "new.value" } else { "value" };
            let initializer = format!("  @snapshot = {call}\n");
            let source = format!(
                "class Counter\n  def {receiver}value\n    11\n  end\n{}end\n\
                 class Counter\n  def {receiver}value\n    42\n  end\n{}end\n",
                if initializer_first { initializer.as_str() } else { "" },
                if initializer_first { "" } else { initializer.as_str() },
            );
            let mut app = roundhouse::App::default();
            app.library_classes = roundhouse::ingest::ingest_library_classes(
                source.as_bytes(), "counter.rb",
            ).unwrap();
            assert_eq!(app.library_classes.len(), 2, "reopens must stay separate");
            let error = roundhouse::project::target_files(
                &app, std::path::Path::new("."), roundhouse::project::BuildTarget::Spinel,
            ).err().expect("reopened class bypasses the method-redefinition guard");
            assert_eq!(error, "class-instance-variable initialization with method redefinition is not supported (spinel): Counter.value");
        }
    }
}

#[test]
fn spinel_keeps_reopens_without_conflicting_state_methods() {
    for source in [
        "class Counter; @value = 1; def self.value; 11; end; end; \
         class Counter; def value; 42; end; end",
        "class Counter; @value = 1; def self.value; 11; end; end; \
         class OtherCounter; def self.value; 42; end; end",
        "class Counter; def self.value; 11; end; end; \
         class Counter; def self.value; 42; end; end",
        "class Counter; @value = 1; end; \
         class OtherCounter; def self.value; 11; end; end; \
         class OtherCounter; def self.value; 42; end; end",
    ] {
        let mut app = roundhouse::App::default();
        app.library_classes = roundhouse::ingest::ingest_library_classes(
            source.as_bytes(), "counter.rb",
        ).unwrap();
        let result = roundhouse::project::target_files(
            &app, std::path::Path::new("."), roundhouse::project::BuildTarget::Spinel,
        );
        assert!(result.is_ok(), "distinct receivers/classes or stateless reopens are supported: {}", result.err().unwrap());
    }
}

fn once_initialized_app() -> emit_and_run::Overlay {
    app()
        .write("app/services/initializer_tally.rb", r#"class InitializerTally
  @value = 0
  def self.advance
    @value += 1
    @value
  end
  def self.value
    @value
  end
end
"#)
        .write("app/services/once_initialized.rb", r#"class OnceInitialized
  @value = InitializerTally.advance
  FIRST = @value
  @value = InitializerTally.advance
  SECOND = @value
  def self.value
    @value
  end
end
"#)
}

const ONCE_INITIALIZED: &str = r#"
raise "first initializer reordered" unless OnceInitialized::FIRST == 1
raise "second initializer reordered" unless OnceInitialized::SECOND == 2
raise "initializer rendered twice" unless OnceInitialized.value == 2
raise "initializer side effects repeated" unless InitializerTally.value == 2
require_relative "app/models/once_initialized"
raise "require repeated initialization" unless InitializerTally.value == 2
puts "initializers run once in source order"
"#;

#[test]
fn class_instance_state_initializers_run_once() {
    once_initialized_app().run_ruby(ONCE_INITIALIZED).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn class_instance_state_initializers_run_once_natively() {
    once_initialized_app().run_spinel(ONCE_INITIALIZED).assert_passes();
}
