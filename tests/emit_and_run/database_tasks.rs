use super::emit_and_run;

const MODEL: &str = r#"
class Room::MessagesCount
  NAMES = ["messages_ai_rooms_messages_count", "messages_ad_rooms_messages_count", "messages_au_rooms_messages_count"]

  def self.ensure!(connection = ActiveRecord::Base.connection)
    return nil unless connection.adapter_name.to_s.downcase.include?("sqlite")
    return nil unless connection.data_source_exists?("rooms")
    return nil unless connection.column_exists?("rooms", "messages_count")
    return nil if triggers_installed?(connection)
    uninstall!(connection)
    connection.execute("UPDATE rooms SET messages_count = (SELECT COUNT(*) FROM messages WHERE messages.room_id = rooms.id)")
    connection.execute("CREATE TRIGGER messages_ai_rooms_messages_count AFTER INSERT ON messages BEGIN UPDATE rooms SET messages_count = messages_count + 1 WHERE id = NEW.room_id; END")
    connection.execute("CREATE TRIGGER messages_ad_rooms_messages_count AFTER DELETE ON messages BEGIN UPDATE rooms SET messages_count = messages_count - 1 WHERE id = OLD.room_id; END")
    connection.execute("CREATE TRIGGER messages_au_rooms_messages_count AFTER UPDATE OF room_id ON messages BEGIN UPDATE rooms SET messages_count = messages_count - 1 WHERE id = OLD.room_id; UPDATE rooms SET messages_count = messages_count + 1 WHERE id = NEW.room_id; END")
    nil
  end

  def self.uninstall!(connection = ActiveRecord::Base.connection)
    NAMES.each do |name|
      connection.execute("DROP TRIGGER IF EXISTS #{name}")
    end
    nil
  end

  def self.triggers_installed?(connection = ActiveRecord::Base.connection)
    NAMES.all? do |name|
      !connection.select_value("SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND name = #{connection.quote(name)}").nil?
    end
  end
end
"#;

const RAKE: &str = r#"
module RoomMessagesCountDatabaseTasks
  def load_schema(...)
    super.tap { repair_room_messages_count }
  end

  def migrate(...)
    super.tap { repair_room_messages_count }
  end

  def repair_room_messages_count
    migration_connection_pool.with_connection do |connection|
      Room::MessagesCount.ensure!(connection)
    end
  end
end

ActiveRecord::Tasks::DatabaseTasks.singleton_class.prepend(RoomMessagesCountDatabaseTasks)

namespace :room_messages_count do
  task ensure: :environment do
    ActiveRecord::Base.connection_pool.with_connection do |connection|
      Room::MessagesCount.ensure!(connection)
    end
  end
end
"#;

const TEST: &str = r##"
require "test_helper"

Rails.application.load_tasks unless Rake::Task.task_defined?("room_messages_count:ensure")

class Room::MessagesCountTasksTest < ActiveSupport::TestCase
  self.use_transactional_tests = false

  setup do
    @directory = Dir.mktmpdir("campfire-counter-tasks")
    @schema = File.join(@directory, "schema.rb")
    @migrations = File.join(@directory, "migrations")
    Dir.mkdir(@migrations)
    File.write(@schema, <<~RUBY)
      ActiveRecord::Schema[8.2].define(version: 0) do
        create_table :rooms, force: true do |t|
          t.integer :messages_count, default: 0, null: false
        end
        create_table :messages, force: true do |t|
          t.integer :room_id
          t.string :client_message_id
        end
      end
    RUBY
    @tasks = ActiveRecord::Tasks::DatabaseTasks
    @original_database_url = ENV.delete("DATABASE_URL")
    @original_environment = @tasks.env
    @original_configurations = ActiveRecord::Base.configurations
    @original_migrations = @tasks.migrations_paths
    @original_dump = ActiveRecord.dump_schema_after_migration
    @original_seed_loader = @tasks.seed_loader
    @tasks.migrations_paths = [ @migrations ]
    ActiveRecord.dump_schema_after_migration = false
  end

  teardown do
    ActiveRecord::Base.configurations = @original_configurations
    ENV["DATABASE_URL"] = @original_database_url if @original_database_url
    @tasks.env = @original_environment
    @tasks.migrations_paths = @original_migrations
    @tasks.seed_loader = @original_seed_loader
    ActiveRecord.dump_schema_after_migration = @original_dump
    FileUtils.remove_entry(@directory)
  end

  test "development schema task repairs both databases before restoring its original pool" do
    development = configuration("development")
    test = configuration("test")
    ActiveRecord::Base.configurations = { "development" => { "primary" => development.configuration_hash }, "test" => { "primary" => test.configuration_hash } }
    @tasks.env = "development"

    @tasks.with_temporary_connection(development) do |connection|
      @tasks.load_schema(development)
      original_path = connection.pool.db_config.database

      2.times do
        task = Rake::Task["db:schema:load"]
        task.all_prerequisite_tasks.each(&:reenable)
        task.reenable
        task.invoke

        raise "pool not restored" unless ActiveRecord::Base.connection_db_config.database == original_path
        raise "development triggers missing" unless Room::MessagesCount.triggers_installed?(connection)
        @tasks.with_temporary_connection(test) do |test_connection|
          raise "test triggers missing" unless Room::MessagesCount.triggers_installed?(test_connection)
          assert_counter_tracks_insert(test_connection)
        end
      end
    end
  end

  test "test schema task repairs the test database selected by purge" do
    config = configuration("test")
    ActiveRecord::Base.configurations = { "test" => { "primary" => config.configuration_hash } }

    @tasks.with_temporary_connection(config) do |connection|
      task = Rake::Task["db:test:load_schema"]
      task.all_prerequisite_tasks.each(&:reenable)
      task.reenable
      task.invoke
      raise "purged database has no triggers" unless Room::MessagesCount.triggers_installed?(connection)
      assert_counter_tracks_insert(connection)
    end
  end

  test "prepare installs schema triggers before loading seeds" do
    config = configuration("test")
    ActiveRecord::Base.configurations = { "test" => { "primary" => config.configuration_hash } }
    seeded = false
    loader = Object.new
    loader.define_singleton_method(:load_seed) do
      connection = ActiveRecord::Base.connection
      raise "seeds ran without counter triggers" unless Room::MessagesCount.triggers_installed?(connection)
      connection.execute("INSERT INTO rooms (id) VALUES (1)")
      connection.execute("INSERT INTO messages (room_id) VALUES (1), (1)")
      raise "seed counter is stale" unless connection.select_value("SELECT messages_count FROM rooms WHERE id = 1") == 2
      seeded = true
    end
    @tasks.seed_loader = loader

    @tasks.with_temporary_connection(config) do |connection|
      @tasks.prepare_all
      raise "seeds did not run" unless seeded
      raise "triggers missing after prepare" unless Room::MessagesCount.triggers_installed?(connection)
      raise "counter mismatch" unless connection.select_value("SELECT messages_count FROM rooms WHERE id = 1") == 2
    end
  end

  test "schema load marks earlier migrations applied" do
    config = configuration("test")
    File.write(File.join(@migrations, "20200101000000_create_rooms.rb"), <<~RUBY)
      class CreateRooms < ActiveRecord::Migration[8.2]
        def change
          create_table :rooms do |t|
            t.integer :messages_count
          end
        end
      end
    RUBY
    File.write(@schema, <<~RUBY)
      ActiveRecord::Schema[8.2].define(version: 2020_01_01_000000) do
        create_table :rooms, force: true do |t|
          t.integer :messages_count, default: 0, null: false
        end
        create_table :messages, force: true do |t|
          t.integer :room_id
          t.string :client_message_id
        end
      end
    RUBY

    @tasks.with_temporary_connection(config) do |connection|
      @tasks.load_schema(config)
      recorded = connection.select_value("SELECT version FROM schema_migrations WHERE version = '20200101000000'")
      raise "schema load left the older migration pending" if recorded.nil?
      @tasks.migrate
      raise "rooms table missing" unless connection.data_source_exists?("rooms")
    end
  end

  test "refusing a schema dump does not create schema_migrations" do
    config = configuration("test")
    File.write(File.join(@migrations, "20261008000100_change_counter_task_message_column.rb"), <<~RUBY)
      class ChangeCounterTaskMessageColumn < ActiveRecord::Migration[8.2]
        def change
          change_column :messages, :client_message_id, :text
        end
      end
    RUBY
    ActiveRecord.dump_schema_after_migration = true

    @tasks.with_temporary_connection(config) do |connection|
      # The process database has no task config. migrate must not
      # invent schema_migrations before it refuses the dump, or a
      # later prepare treats the empty table as an initialized database.
      @tasks.instance_variable_set(:@current, nil)
      refused = false
      begin
        @tasks.migrate
      rescue RuntimeError => e
        refused = e.message.include?("schema dump after migration is not supported")
      end
      raise "migrate did not refuse the dump" unless refused
      raise "refused migrate created schema_migrations" if connection.data_source_exists?("schema_migrations")
      @tasks.load_schema(config)
      raise "schema load skipped after the refusal" unless connection.data_source_exists?("rooms")
    end
  end

  test "a uuid primary key does not load or record the schema version" do
    config = configuration("test")
    File.write(@schema, <<~RUBY)
      ActiveRecord::Schema[8.2].define(version: 2020_01_01_000000) do
        create_table :rooms, id: :uuid, force: true do |t|
          t.integer :messages_count, default: 0, null: false
        end
      end
    RUBY

    @tasks.with_temporary_connection(config) do |connection|
      refused = false
      begin
        @tasks.load_schema(config)
      rescue RuntimeError => e
        refused = e.message.include?("unsupported schema statement")
      end
      raise "uuid primary key was accepted" unless refused
      raise "uuid primary key created rooms" if connection.data_source_exists?("rooms")
      raise "uuid primary key recorded a version" if connection.data_source_exists?("schema_migrations")
    end
  end

  test "migrate repairs rebuilt message tables and repeats repair with no new migration" do
    config = configuration("test")
    File.write(File.join(@migrations, "20261008000100_change_counter_task_message_column.rb"), <<~RUBY)
      class ChangeCounterTaskMessageColumn < ActiveRecord::Migration[8.2]
        def change
          change_column :messages, :client_message_id, :text
        end
      end
    RUBY

    @tasks.with_temporary_connection(config) do |connection|
      @tasks.load_schema(config)
      Room::MessagesCount.ensure!(connection)
      assert_counter_tracks_insert(connection)
      @tasks.migrate
      raise "triggers missing after migrate" unless Room::MessagesCount.triggers_installed?(connection)
      assert_counter_tracks_insert(connection)

      Room::MessagesCount.uninstall!(connection)
      connection.execute("INSERT INTO messages (room_id) VALUES (1)")
      counted = connection.select_value("SELECT COUNT(*) FROM messages WHERE room_id = 1")
      stored = connection.select_value("SELECT messages_count FROM rooms WHERE id = 1")
      raise "uninstall did not stop the counter" if counted == stored
      @tasks.migrate
      raise "second migrate did not repair" unless Room::MessagesCount.triggers_installed?(connection)
      counted = connection.select_value("SELECT COUNT(*) FROM messages WHERE room_id = 1")
      stored = connection.select_value("SELECT messages_count FROM rooms WHERE id = 1")
      raise "second migrate did not backfill" unless counted == stored
    end
  end

  private
    def configuration(environment)
      ActiveRecord::DatabaseConfigurations::HashConfig.new(environment, "primary", {
        adapter: "sqlite3", database: File.join(@directory, "#{environment}.sqlite3"),
        default_transaction_mode: :immediate, schema_dump: @schema,
        migrations_paths: [ @migrations ]
      })
    end

    def assert_counter_tracks_insert(connection)
      connection.execute("INSERT INTO rooms (id) VALUES (1) ON CONFLICT DO NOTHING")
      before = connection.select_value("SELECT messages_count FROM rooms WHERE id = 1")
      connection.execute("INSERT INTO messages (room_id) VALUES (1)")
      after = connection.select_value("SELECT messages_count FROM rooms WHERE id = 1")
      raise "counter did not advance" unless after == before + 1
    end
end
"##;

#[test]
fn database_tasks_repair_counters_on_ruby() {
    emit_and_run::real_blog()
        .write("app/models/room/messages_count.rb", MODEL)
        .write("lib/tasks/room_messages_count.rake", RAKE)
        .write("test/models/room/messages_count_tasks_test.rb", TEST)
        .run_test("test/models/room_messages_count_tasks_test.rb")
        .assert_passes();
}
