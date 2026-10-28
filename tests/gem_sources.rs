//! A gem compiled from source (a PATH gem) is the app's code: it runs in
//! the emitted tree, and its RBI is not the boundary.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "PATH\n  remote: vendor/gems/widget-1.0.0\n  specs:\n    widget (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  widget!\n";

const WIDGET_RBI: &str = "# typed: true\nmodule Widget\n  def self.answer; end\nend\n";

fn widget_app(lock: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile.lock", lock)
        .write("sorbet/rbi/gems/widget@1.0.0.rbi", WIDGET_RBI)
        .write("vendor/gems/widget-1.0.0/lib/widget.rb", "require \"widget/answer\"\n\nmodule Widget\n  def self.answer\n    Answer.new.value\n  end\nend\n")
        .write("vendor/gems/widget-1.0.0/lib/widget/answer.rb", "module Widget\n  class Answer\n    def value\n      42\n    end\n  end\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("lib/oracle.rb", "require \"widget\"\n\nclass Oracle\n  def ask\n    Widget.answer\n  end\nend\n")
        .write("test/oracle_test.rb", r#"require "test_helper"
require "oracle"
class OracleTest < ActiveSupport::TestCase
  test "the carried gem answers" do
    assert_equal 42, Oracle.new.ask
  end
end
"#)
}

const CARRIED: &str = "test_paths:\n  - test\n";

#[test]
fn a_carried_gem_runs_in_emitted_ruby() {
    widget_app(LOCK).run_test("test/models/oracle_test.rb").assert_passes();
}

#[test]
fn a_carried_gem_is_not_refused_as_the_gems() {
    let (_, errors) = widget_app(LOCK).emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// The same gem from the registry, not the app's tree, stays behind its RBI.
const REGISTRY_LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n    widget (1.0.0)\n\nDEPENDENCIES\n  activesupport\n  widget\n";

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn without_the_path_source_the_gem_stays_behind_its_rbi() {
    let (_, errors) = widget_app(REGISTRY_LOCK).emit(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("Widget is declared by the widget gem")), "{errors:#?}");
}

/// graphql-kit 0.20.3 requires cloud-kit inside
/// `begin ... rescue LoadError end` and runs on without it. The boot's
/// load of the gem source's dependencies keeps that rescue for that
/// require, and only for it.
#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_gem_sources_rescued_require_stays_optional() {
    const WIDGET: &str = "require \"set\"\nbegin\n  require \"rh_absent_optional_feature\"\nrescue LoadError\nend\nrequire \"widget/answer\"\n\nmodule Widget\n  def self.answer\n    Answer.new.value\n  end\nend\n";
    let app = || widget_app(LOCK).write("vendor/gems/widget-1.0.0/lib/widget.rb", WIDGET);
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let facades = std::fs::read_to_string(emitted.join("runtime/gem_facades.rb")).unwrap();
    assert!(
        facades.contains("[[\"set\", false], [\"rh_absent_optional_feature\", true]].each do |feature, optional|"),
        "{facades}"
    );
    app().run_test("test/models/oracle_test.rb").assert_passes();
}

/// stream-kit 5.2.3's classic entry runs
/// `require "stream-kit/legacy/lib/rspec_matcher" if defined?(RSpec)`,
/// and the matcher requires `rspec/expectations`. A Minitest boot never
/// loads it (RSpec is not defined), so rspec need not be installed. The
/// boot's load of the gem source's dependencies left the condition out
/// and raised LoadError. A conditional require is the carried program's.
#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_gem_sources_conditional_require_loads_only_where_its_condition_holds() {
    const WIDGET: &str = "require \"set\"\nrequire \"widget/answer\"\nrequire \"widget/matcher\" if defined?(RSpec)\nrequire \"rh_absent_unless_feature\" unless defined?(Set)\n\nmodule Widget\n  def self.answer\n    Answer.new.value\n  end\nend\n";
    const MATCHER: &str = "require \"rh_absent_test_group_feature\"\n\nmodule Widget\n  MATCHER = true\nend\n";
    let app = || {
        widget_app(LOCK)
            .write("vendor/gems/widget-1.0.0/lib/widget.rb", WIDGET)
            .write("vendor/gems/widget-1.0.0/lib/widget/matcher.rb", MATCHER)
    };
    let (emitted, _) = app().emit(BuildTarget::Ruby);
    let facades = std::fs::read_to_string(emitted.join("runtime/gem_facades.rb")).unwrap();
    assert!(facades.contains("[\"set\"].each do |feature|"), "{facades}");
    assert!(!facades.contains("rh_absent_"), "{facades}");
    app().run_test("test/models/oracle_test.rb").assert_passes();
}

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_class_body_on_a_native_base_reached_through_a_gem_source_replays() {
    const LOCK: &str = "PATH\n  remote: vendor/gems/widget-1.0.0\n  specs:\n    widget (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n    concurrent-ruby (1.3.8)\n\nDEPENDENCIES\n  activesupport\n  concurrent-ruby\n  widget!\n";
    const RBI: &str = "# typed: true\nmodule Concurrent\n  module Synchronization\n    class Object\n      def self.safe_initialization!; end\n      def self.safe_initialization?; end\n    end\n  end\n\n  module Concern\n    module Logging\n      def log(level, progname, message = T.unsafe(nil), &block); end\n    end\n  end\nend\n";
    const WIDGET: &str = "require \"concurrent\"\nrequire \"widget/box\"\nrequire \"widget/taggable\"\n\nmodule Widget\nend\n";
    const BOX: &str = "module Widget\n  class Box < Concurrent::Synchronization::Object\n    def self.kind = \"widget\"\n  end\nend\n";
    const TAGGABLE: &str = "module Widget\n  module Taggable\n    include Concurrent::Concern::Logging\n\n    def self.included(base)\n      base.extend(ClassMethods)\n    end\n\n    module ClassMethods\n      def tag(value = nil)\n        @tag = value if value\n        @tag\n      end\n    end\n  end\nend\n";
    const APP_BOX: &str = "require \"widget\"\n\nclass AppBox < Widget::Box\n  safe_initialization!\n\n  def self.greet = \"hello\"\nend\n";
    const APP_TAGGED: &str = "require \"widget\"\n\nmodule AppTagged\n  include Widget::Taggable\n  tag \"app\"\nend\n";
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile.lock", LOCK)
        .write("sorbet/rbi/gems/concurrent-ruby@1.3.8.rbi", RBI)
        .write("vendor/gems/widget-1.0.0/lib/widget.rb", WIDGET)
        .write("vendor/gems/widget-1.0.0/lib/widget/box.rb", BOX)
        .write("vendor/gems/widget-1.0.0/lib/widget/taggable.rb", TAGGABLE)
        .write("lib/app_box.rb", APP_BOX)
        .write("lib/app_tagged.rb", APP_TAGGED)
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/box_test.rb", r#"require "test_helper"
require "app_box"
require "app_tagged"
class BoxTest < ActiveSupport::TestCase
  test "the native gem's class-body DSL ran" do
    assert_equal true, AppBox.safe_initialization?
    assert_nil Widget::Box.safe_initialization?
    assert_equal "hello", AppBox.greet
    assert_equal "widget", AppBox.kind
    assert_equal "app", AppTagged.tag
  end
end
"#)
        .run_test("test/models/box_test.rb");
    run.assert_passes();
}

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn mixes_in_class_methods_extends_each_includer() {
    // sorbet's `mixes_in_class_methods` is behavior: every `include` of
    // the module extends the includer. graphql-kit's BaseInterface
    // gets `GraphQL::Schema::Interface::DefinitionMethods` onto each
    // implementing type this way. It was dropped as an unmodeled call
    // (and `extend T::Helpers` with it), so `AppThing.describe` was a
    // NoMethodError. `requires_ancestor`'s block never runs: sorbet-runtime
    // 0.6.13502 defines the method with an empty body. Native oracle
    // (ruby -rsorbet-runtime, same files): ["described AppThing", :extra,
    // 1, false, true].
    const DESCRIBABLE: &str = "module Widget\n  module Describable\n    extend T::Helpers\n    requires_ancestor { Widget::NotDefinedAnywhere }\n\n    module ClassMethods\n      def describe = \"described #{name}\"\n    end\n\n    module Extra\n      def extra = :extra\n    end\n\n    mixes_in_class_methods(ClassMethods)\n    mixes_in_class_methods Extra\n  end\nend\n";
    const APP_MIXIN: &str = "module AppMixin\n  extend T::Helpers\n\n  module Counting\n    def count = 1\n  end\n\n  mixes_in_class_methods(Counting)\nend\n";
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile.lock", LOCK)
        .write("vendor/gems/widget-1.0.0/lib/widget.rb", "require \"widget/describable\"\n\nmodule Widget\nend\n")
        .write("vendor/gems/widget-1.0.0/lib/widget/describable.rb", DESCRIBABLE)
        .write("lib/app_mixin.rb", APP_MIXIN)
        .write("lib/app_thing.rb", "require \"widget\"\n\nclass AppThing\n  include Widget::Describable\nend\n")
        .write("lib/app_other.rb", "require \"app_mixin\"\n\nclass AppOther\n  include AppMixin\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/mixin_test.rb", r#"require "test_helper"
require "app_thing"
require "app_other"
class MixinTest < ActiveSupport::TestCase
  test "each includer is extended with the mixed-in class methods" do
    assert_equal "described AppThing", AppThing.describe
    assert_equal :extra, AppThing.extra
    assert_equal 1, AppOther.count
    assert_equal false, Widget::Describable.respond_to?(:describe)
    assert_equal true, AppThing.singleton_class.include?(Widget::Describable::ClassMethods)
  end
end
"#)
        .run_test("test/models/mixin_test.rb");
    run.assert_passes();
}

#[test]
fn a_carried_engine_gems_app_models_autoload() {
    const LOCK: &str = "PATH\n  remote: vendor/gems/widget-1.0.0\n  specs:\n    widget (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  widget!\n";
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile.lock", LOCK)
        .write("vendor/gems/widget-1.0.0/lib/widget.rb", "require \"widget/engine\"\n\nmodule Widget\nend\n")
        .write("vendor/gems/widget-1.0.0/lib/widget/engine.rb", "module Widget\n  class Engine < ::Rails::Engine\n    isolate_namespace Widget\n  end\nend\n")
        .write("vendor/gems/widget-1.0.0/app/models/widget/null_builder.rb", "module Widget\n  class NullBuilder\n    def collection\n      []\n    end\n\n    def kind\n      \"null\"\n    end\n  end\nend\n")
        .write("vendor/gems/widget-1.0.0/app/models/widget/task.rb", "module Widget\n  class Task\n    def self.named(name)\n      \"task:#{name}\"\n    end\n  end\nend\n")
        .write("app/models/widget/task/csv_collection.rb", "module Widget\n  class Task\n    module CsvCollection\n      class Builder < Widget::NullBuilder\n        def collection\n          [\"a\", \"b\"]\n        end\n      end\n    end\n  end\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/builder_test.rb", r#"require "test_helper"
class BuilderTest < ActiveSupport::TestCase
  test "the engine gem's app models load" do
    builder = Widget::Task::CsvCollection::Builder.new
    assert_equal ["a", "b"], builder.collection
    assert_equal "null", builder.kind
    assert_equal "task:x", Widget::Task.named("x")
  end
end
"#)
        .run_test("test/models/builder_test.rb");
    run.assert_passes();
}

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_class_body_on_a_native_base_the_app_also_reopens_replays() {
    const LOCK: &str = "PATH\n  remote: vendor/gems/widget-1.0.0\n  specs:\n    widget (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n    concurrent-ruby (1.3.8)\n\nDEPENDENCIES\n  activesupport\n  concurrent-ruby\n  widget!\n";
    const RBI: &str = "# typed: true\nmodule Concurrent\n  module Synchronization\n    class Object\n      def self.safe_initialization!; end\n      def self.safe_initialization?; end\n    end\n  end\nend\n";
    const WIDGET: &str = "require \"concurrent\"\nrequire \"widget/box\"\n\nmodule Widget\nend\n";
    const BOX: &str = "module Widget\n  class Box < Concurrent::Synchronization::Object\n  end\nend\n";
    const REOPEN: &str = "require \"concurrent\"\n\nmodule Concurrent\n  module Synchronization\n    class Object\n      def self.reopened = :yes\n    end\n  end\nend\n";
    const APP_BOX: &str = "require \"widget\"\nrequire \"concurrent_reopen\"\n\nclass AppBox < Widget::Box\n  safe_initialization!\nend\n";
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile.lock", LOCK)
        .write("sorbet/rbi/gems/concurrent-ruby@1.3.8.rbi", RBI)
        .write("vendor/gems/widget-1.0.0/lib/widget.rb", WIDGET)
        .write("vendor/gems/widget-1.0.0/lib/widget/box.rb", BOX)
        .write("lib/concurrent_reopen.rb", REOPEN)
        .write("lib/app_box.rb", APP_BOX)
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/box_test.rb", r#"require "test_helper"
require "app_box"
class BoxTest < ActiveSupport::TestCase
  test "the native gem's class-body DSL ran through the reopened base" do
    assert_equal true, AppBox.safe_initialization?
    assert_equal :yes, AppBox.reopened
  end
end
"#)
        .run_test("test/models/box_test.rb");
    run.assert_passes();
}

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_concern_without_hook_blocks_replays_its_gem_dsl_body() {
    const LOCK: &str = "PATH\n  remote: vendor/gems/widget-1.0.0\n  specs:\n    widget (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n    concurrent-ruby (1.3.8)\n\nDEPENDENCIES\n  activesupport\n  concurrent-ruby\n  widget!\n";
    const RBI: &str = "# typed: true\nmodule Concurrent\n  module Concern\n    module Logging\n      def log(level, progname, message = T.unsafe(nil), &block); end\n    end\n  end\nend\n";
    const TAGGABLE: &str = "module Widget\n  module Taggable\n    include Concurrent::Concern::Logging\n\n    def self.included(base)\n      base.extend(ClassMethods)\n    end\n\n    module ClassMethods\n      def tag(value = nil)\n        @tag = value if value\n        @tag\n      end\n    end\n  end\nend\n";
    const APP_TAGGED: &str = "require \"widget\"\n\nmodule AppTagged\n  extend ActiveSupport::Concern\n  include Widget::Taggable\n  tag \"app\"\nend\n";
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile.lock", LOCK)
        .write("sorbet/rbi/gems/concurrent-ruby@1.3.8.rbi", RBI)
        .write("vendor/gems/widget-1.0.0/lib/widget.rb", "require \"concurrent\"\nrequire \"widget/taggable\"\n\nmodule Widget\nend\n")
        .write("vendor/gems/widget-1.0.0/lib/widget/taggable.rb", TAGGABLE)
        .write("lib/app_tagged.rb", APP_TAGGED)
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/tagged_test.rb", r#"require "test_helper"
require "app_tagged"
class TaggedTest < ActiveSupport::TestCase
  test "the concern's gem DSL body ran" do
    assert_equal "app", AppTagged.tag
  end
end
"#)
        .run_test("test/models/tagged_test.rb");
    run.assert_passes();
}

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_concerns_constant_loop_over_native_class_side_calls_runs_on_the_includer() {
    const LOCK: &str = "PATH\n  remote: vendor/gems/widget-1.0.0\n  specs:\n    widget (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n    concurrent-ruby (1.3.8)\n\nDEPENDENCIES\n  activesupport\n  concurrent-ruby\n  widget!\n";
    const RBI: &str = "# typed: true\nmodule Concurrent\n  module Synchronization\n    class Object\n      def self.safe_initialization!; end\n      def self.safe_initialization?; end\n    end\n  end\nend\n";
    const WIDGET: &str = "require \"concurrent\"\nrequire \"widget/box\"\n\nmodule Widget\nend\n";
    const BOX: &str = "module Widget\n  class Box < Concurrent::Synchronization::Object\n  end\nend\n";
    const SAFE: &str = "module AppSafe\n  extend ActiveSupport::Concern\n\n  SOURCES = [:a, :b].freeze\n\n  included do\n    AppSafe::SOURCES.each do |_source|\n      safe_initialization!\n    end\n  end\nend\n";
    const APP_BOX: &str = "require \"widget\"\nrequire \"app_safe\"\n\nclass AppBox < Widget::Box\n  include AppSafe\nend\n";
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile.lock", LOCK)
        .write("sorbet/rbi/gems/concurrent-ruby@1.3.8.rbi", RBI)
        .write("vendor/gems/widget-1.0.0/lib/widget.rb", WIDGET)
        .write("vendor/gems/widget-1.0.0/lib/widget/box.rb", BOX)
        .write("lib/app_safe.rb", SAFE)
        .write("lib/app_box.rb", APP_BOX)
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/box_test.rb", r#"require "test_helper"
require "app_box"
class BoxTest < ActiveSupport::TestCase
  test "the concern's loop ran on the includer" do
    assert_equal true, AppBox.safe_initialization?
  end
end
"#)
        .run_test("test/models/box_test.rb");
    run.assert_passes();
}
