# frozen_string_literal: true
require_relative "../test_helper"
require "tmpdir"
require "active_job"
require "spinel/active_support_cache"

class FrameworkSupportTest < Minitest::Test
  def test_isolated_execution_state_is_per_thread
    ActiveSupport::IsolatedExecutionState[:box] = { "n" => 1 }
    assert_equal({ "n" => 1 }, ActiveSupport::IsolatedExecutionState[:box])
    other = nil
    Thread.new { other = ActiveSupport::IsolatedExecutionState[:box] }.join
    assert_nil other
  end

  def test_notifications_fan_out_and_capture_sql
    seen = []
    subscriber = ActiveSupport::Notifications.subscribe(/demo/) do |name, _, _, _, payload|
      seen << [name, payload[:key]]
    end
    ActiveSupport::Notifications.instrument("demo.event", { key: "a" }) { "ran" }
    ActiveSupport::Notifications.unsubscribe(subscriber)
    assert_equal [["demo.event", "a"]], seen

    events = []
    ActiveSupport::Notifications.subscribe("once") { |event| events << event.payload[:key] }
    ran = ActiveSupport::Notifications.instrument("once", { key: "b" }) { "ran" }
    assert_equal "ran", ran
    assert_equal ["b"], events
    done = ActiveSupport::Notifications.subscribed(->(_, _, _, _, _) {}, "other", monotonic: true) { "done" }
    assert_equal "done", done

    event = nil
    ActiveSupport::Notifications.subscribe("clock") { |found| event = found }
    ActiveSupport::Notifications.instrument("clock") { "ticked" }
    assert_equal "clock", event.name
    assert_equal true, event.duration >= 0
    assert_equal true, event.end.to_f >= event.time.to_f
  end

  def test_null_store_does_not_keep_a_value
    store = ActiveSupport::Cache::NullStore.new
    assert_nil store.read("k")
    assert_equal true, store.write("k", "v")
    assert_nil store.read("k")
    assert_equal false, store.exist?("k")
    assert_equal "computed", store.fetch("k") { "computed" }
    assert_nil store.read("k")
    assert_nil store.read_str("k")
    assert_equal "fragment", store.fetch_str("k", 0) { "fragment" }
    assert_nil store.read_str("k")
  end

  def test_configuration_file_parses_yaml_and_erb
    path = File.join(Dir.tmpdir, "rh-config-#{Process.pid}.yml")
    File.write(path, "size: <%= 1 + 1 %>\n")
    loaded = ActiveSupport::ConfigurationFile.parse(path)
    assert_equal 2, loaded["size"]
    File.write(path, "adapter: :redis\n")
    assert_equal :redis, ActiveSupport::ConfigurationFile.parse(path)["adapter"]
  ensure
    File.delete(path) if path && File.exist?(path)
  end

  def test_subscribed_sql_accepts_an_event
    skip "Db is not loaded" unless defined?(Db)
    sqls = []
    result = ActiveSupport::Notifications.subscribed(->(event) { sqls << event.payload[:sql] }, "sql.active_record") do
      Db.record_query("SELECT 1")
      "done"
    end
    assert_equal "done", result
    assert_equal ["SELECT 1"], sqls
  end

  def test_sql_reentry_guard_is_per_thread
    seen = []
    subscriber = ActiveSupport::Notifications.subscribe("sql.active_record") { |*args| seen << args[4][:sql] }
    Thread.current[:active_support_publishing_sql] = true
    ActiveSupport::Notifications.sql("hidden")
    Thread.new { ActiveSupport::Notifications.sql("visible") }.join
    assert_equal ["visible"], seen
  ensure
    Thread.current[:active_support_publishing_sql] = false
    ActiveSupport::Notifications.unsubscribe(subscriber) if subscriber
  end

  def test_enqueue_error_and_continuable_steps
    error = assert_raises(ActiveJob::EnqueueError) { raise ActiveJob::EnqueueError, "full" }
    assert_equal "full", error.message

    job = Class.new do
      include ActiveJob::Continuable
      def run
        first = step(:one) { 1 }
        second = step("one") { 2 }
        [first, second]
      end
    end
    assert_equal [1, nil], job.new.run
  end
end
