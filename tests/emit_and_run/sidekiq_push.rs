//! `queue_adapter :sidekiq` on the Ruby-family lanes: what an app that
//! sets it, and has a payload job, gets in its emitted tree. A real
//! Redis client in `runtime/sidekiq_redis.rb` (spinel-redis on spinel,
//! redis-rb on ruby and jruby), the package or gem that provides it,
//! and `JobRegistry.sidekiq?` true. Every other app keeps the raising
//! façade and no dependency.

use roundhouse::project::BuildTarget;

use super::emit_and_run;

const JOB: &str = r##"class ArticleJob < ApplicationJob
  queue_as :default

  def perform(article)
    article.title
  end
end
"##;

const ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_job
    ArticleJob.perform_later(self)
  end
"##;

const SIDEKIQ: &str = "  config.active_job.queue_adapter = :sidekiq\n";

fn on_sidekiq() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
}

fn read(emitted: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn a_spinel_tree_on_sidekiq_gets_spinel_redis() {
    let (emitted, errors) = on_sidekiq().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let client = read(&emitted, "runtime/sidekiq_redis.rb");
    assert!(
        client.contains("require \"redis\"")
            && client.contains(
                "Redis.new(url: url, connect_timeout: CONNECT_TIMEOUT, read_timeout: READ_TIMEOUT)"
            ),
        "{client}"
    );
    assert!(!emitted.join("runtime/sidekiq_redis.rbs").exists(), "the façade's sidecar must drop");
    let manifest = read(&emitted, "spin.toml");
    assert!(
        manifest.contains(
            "redis = { git = \"https://github.com/roundhouse-rb/spinel-redis\", ref = \"v0.3.2\" }"
        ),
        "{manifest}"
    );
    let registry = read(&emitted, "runtime/job_registry.rb");
    assert!(registry.contains("  def self.sidekiq?\n    true\n  end\n"), "{registry}");
}

#[test]
fn ruby_and_jruby_trees_on_sidekiq_get_redis_rb() {
    for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
        let (emitted, errors) = on_sidekiq().emit(target);
        assert!(errors.is_empty(), "{errors:#?}");
        let client = read(&emitted, "runtime/sidekiq_redis.rb");
        assert!(client.contains("require \"redis\""), "{client}");
        let gemfile = read(&emitted, "Gemfile");
        assert!(gemfile.contains("gem \"redis\""), "{gemfile}");
        let registry = read(&emitted, "runtime/job_registry.rb");
        assert!(registry.contains("  def self.sidekiq?\n    true\n  end\n"), "{registry}");
    }
}

#[test]
fn an_app_not_on_sidekiq_keeps_the_facade_and_no_dependency() {
    let plain = || {
        emit_and_run::real_blog()
            .write("app/jobs/article_job.rb", JOB)
            .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
    };
    let (emitted, _) = plain().emit(BuildTarget::Spinel);
    let facade = read(&emitted, "runtime/sidekiq_redis.rb");
    assert!(facade.contains("this tree has no Redis client") && !facade.contains("require"), "{facade}");
    assert!(emitted.join("runtime/sidekiq_redis.rbs").exists());
    assert!(!read(&emitted, "spin.toml").contains("spinel-redis"));
    assert!(read(&emitted, "runtime/job_registry.rb").contains("  def self.sidekiq?\n    false\n  end\n"));

    let (emitted, _) = plain().emit(BuildTarget::Ruby);
    assert!(read(&emitted, "runtime/sidekiq_redis.rb").contains("this tree has no Redis client"));
    assert!(!read(&emitted, "Gemfile").contains("gem \"redis\""));
}

/// The worker role: a tree on `:sidekiq` gets the real worker in place of
/// the façade, and the ruby tree its `sidekiq.rb` entry. Every other tree
/// keeps the façade, whose `cli` says the app isn't on Sidekiq, and no
/// `sidekiq.rb`.
#[test]
fn a_tree_on_sidekiq_gets_the_worker_role() {
    let (emitted, errors) = on_sidekiq().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let worker = read(&emitted, "runtime/sidekiq_worker.rb");
    assert!(worker.contains("def self.processor(index)"), "{worker}");
    assert!(!emitted.join("runtime/sidekiq_worker.rbs").exists(), "the façade's sidecar must drop");
    assert!(read(&emitted, "main.rb").contains("SidekiqWorker.cli(ARGV)"));
    assert!(read(&emitted, "runtime/sidekiq_envelope.rbs").contains("def self.with_error"));

    for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
        let (emitted, errors) = on_sidekiq().emit(target);
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(read(&emitted, "runtime/sidekiq_worker.rb").contains("def self.processor(index)"));
        assert!(read(&emitted, "sidekiq.rb").contains("SidekiqWorker.cli(ARGV)"));
    }

    let plain = || {
        emit_and_run::real_blog()
            .write("app/jobs/article_job.rb", JOB)
            .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
    };
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let (emitted, _) = plain().emit(target);
        let facade = read(&emitted, "runtime/sidekiq_worker.rb");
        assert!(facade.contains("doesn't use queue_adapter :sidekiq") && !facade.contains("processor"), "{facade}");
        assert!(!emitted.join("sidekiq.rb").exists());
    }
}

const WORKER_SCRIPT: &str = r##"SidekiqWorker::LOGGING[0] = false
raise "parse" unless SidekiqWorker.parse(["sidekiq", "-q", "critical,3", "-q", "default", "-c", "4"]) == ""
raise "queues #{SidekiqWorker::QUEUES.inspect}" unless SidekiqWorker::QUEUES == ["queue:critical"] * 3 + ["queue:default"]
raise "concurrency" unless SidekiqWorker::CONCURRENCY[0] == 4
ENV.delete("DATABASE_POOL_SIZE")
SidekiqWorker.prepare_pool(["sidekiq", "-c", "10"])
raise "pool #{ENV["DATABASE_POOL_SIZE"]}" unless ENV["DATABASE_POOL_SIZE"] == "12"
article = Article.create!(title: "Worker title", body: "A long enough article body.")
json = JobRegistry.payload_article_job(article)
raise "no outcome" unless ActiveJob.perform_parsed(JSON.parse(json)) == ActiveJob::PERFORM_RAN
article.destroy
raise "gone" unless ActiveJob.perform_parsed(JSON.parse(json)) == ActiveJob::PERFORM_FAILED
raise ActiveJob.failure_class unless ActiveJob.failure_class == "ActiveJob::DeserializationError"
puts "worker role passed"
"##;

/// The emitted ruby tree's worker parses Sidekiq's options, sizes the
/// database pool, and runs the app's own payload job through
/// `perform_parsed`, a gone record failing as DeserializationError.
#[test]
fn the_worker_role_runs_the_apps_jobs() {
    let run = on_sidekiq().run_ruby(WORKER_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("worker role passed"), "{}", run.stdout);
}

#[test]
fn an_app_on_sidekiq_with_no_jobs_gets_no_redis() {
    let (emitted, errors) = emit_and_run::real_blog()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(read(&emitted, "runtime/sidekiq_redis.rb").contains("this tree has no Redis client"));
    assert!(!read(&emitted, "spin.toml").contains("spinel-redis"));
}

const COMMIT_CALLBACK: &str = r#"  validates :title, presence: true

  COMMITTED = []
  after_create_commit :note_committed

  def note_committed
    COMMITTED << title
  end

  def enqueue_job
    ArticleJob.perform_later(self)
  end
"#;

const FAILED_PUSH_SCRIPT: &str = r#"SidekiqRedis.define_singleton_method(:push) { |_url, _queue, _json| raise "redis down" }
begin
  Article.transaction do
    Article.create!(title: "Pushed title", body: "A long enough article body.")
    SidekiqPush.enqueue_after_commit("ArticleJob", "default", 0.0, "{}")
  end
  raise "the failed push was swallowed"
rescue RuntimeError => e
  raise e unless e.message == "redis down"
end
raise "after_commit skipped: #{Article::COMMITTED.inspect}" unless Article::COMMITTED == ["Pushed title"]
Article.transaction do
  Article.create!(title: "Next title", body: "Another long enough body.")
end
raise "after_commit fired again: #{Article::COMMITTED.inspect}" unless Article::COMMITTED == ["Pushed title", "Next title"]
puts "failed push passed"
"#;

/// A held push that fails at COMMIT raises after the data is committed,
/// as an error in a Rails `after_commit` does, but the records'
/// `after_commit` callbacks have run first: none is skipped, and none
/// waits to fire at the thread's next COMMIT. Rails enqueues after the
/// record callbacks too.
#[test]
fn a_failed_push_at_commit_runs_the_record_callbacks_first() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", COMMIT_CALLBACK)
        .run_ruby(FAILED_PUSH_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("failed push passed"), "{}", run.stdout);
}

const RAISING_CALLBACK: &str = r#"  validates :title, presence: true

  after_create_commit :refuse

  def refuse
    raise ArgumentError, "callback failed" if title == "Refused title"
  end
"#;

const RAISING_CALLBACK_SCRIPT: &str = r#"SidekiqPush.start_recording
begin
  Article.transaction do
    Article.create!(title: "Refused title", body: "A long enough article body.")
    SidekiqPush.enqueue_after_commit("ArticleJob", "default", 0.0, "{}")
  end
  raise "the callback's error was swallowed"
rescue ArgumentError => e
  raise e unless e.message == "callback failed"
end
raise "pushed after a raising callback: #{SidekiqPush.recorded.inspect}" unless SidekiqPush.recorded.empty?
raise "still held: #{SidekiqPush.held.length}" unless SidekiqPush.held.empty?
raise "the data was not committed" unless Article.where(title: "Refused title").count == 1
Article.transaction do
  Article.create!(title: "Next title", body: "Another long enough body.")
end
raise "the dropped job was pushed later: #{SidekiqPush.recorded.inspect}" unless SidekiqPush.recorded.empty?
SidekiqPush.stop_recording
puts "raising callback passed"
"#;

/// A record's `after_commit` that raises ends the COMMIT there, as in
/// Rails 8.1, where `after_all_transactions_commit` (the enqueue) runs
/// after the record callbacks: the data stays committed, the caller
/// sees the callback's error, and the held job is dropped, not pushed
/// then or at the thread's next COMMIT.
#[test]
fn a_raising_commit_callback_drops_the_held_push() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", RAISING_CALLBACK)
        .run_ruby(RAISING_CALLBACK_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("raising callback passed"), "{}", run.stdout);
}

const SIDEKIQ_YML: &str = ":concurrency: 3\n:timeout: 9\n:queues:\n  - critical\n  - [default, 2]\n:max_retries: 4\n:dead_max_jobs: 50\n:dead_timeout_in_seconds: 3600\n:poll_interval_average: 2.5\n";

const SIDEKIQ_YML_SCRIPT: &str = r#"SidekiqWorker.parse(["sidekiq"])
raise "concurrency #{SidekiqWorker::CONCURRENCY[0]}" unless SidekiqWorker::CONCURRENCY[0] == 3
raise "timeout #{SidekiqWorker::TIMEOUT[0]}" unless SidekiqWorker::TIMEOUT[0] == 9
raise "queues #{SidekiqWorker::QUEUES.inspect}" unless SidekiqWorker::QUEUES == ["queue:critical", "queue:default", "queue:default"]
raise "weighted" if SidekiqWorker::STRICT[0]
SidekiqWorker.parse(["sidekiq", "-c", "7", "-q", "low", "-t", "1"])
raise "-c #{SidekiqWorker::CONCURRENCY[0]}" unless SidekiqWorker::CONCURRENCY[0] == 7
raise "-t #{SidekiqWorker::TIMEOUT[0]}" unless SidekiqWorker::TIMEOUT[0] == 1
raise "-q #{SidekiqWorker::QUEUES.inspect}" unless SidekiqWorker::QUEUES == ["queue:low"] && SidekiqWorker::STRICT[0]
ENV["RAILS_MAX_THREADS"] = "11"
SidekiqWorker.parse(["sidekiq"])
raise "the file beats RAILS_MAX_THREADS: #{SidekiqWorker::CONCURRENCY[0]}" unless SidekiqWorker::CONCURRENCY[0] == 3
raise "max_retries #{SidekiqWorker.max_retries}" unless SidekiqWorker.max_retries == 4
raise "dead_max_jobs" unless JobRegistry.sidekiq_worker_dead_max_jobs == 50
raise "dead_timeout_in_seconds" unless JobRegistry.sidekiq_worker_dead_timeout == 3600
raise "poll_interval_average" unless JobRegistry.sidekiq_worker_poll_interval_average == 2.5
raise "average_scheduled_poll_interval" unless JobRegistry.sidekiq_worker_scheduled_poll_interval == -1.0
puts "sidekiq.yml passed"
"#;

/// `config/sidekiq.yml` gives the worker its defaults, and the command
/// line wins over them, key by key, as Sidekiq's CLI merges them; the
/// file wins over `RAILS_MAX_THREADS`. The retry limit, the dead set's
/// trims and the poller's interval come from it too; a key it doesn't
/// give stays at Sidekiq's default (-1 in the registry).
#[test]
fn sidekiq_yml_sets_the_worker_defaults() {
    let run = on_sidekiq().write("config/sidekiq.yml", SIDEKIQ_YML).run_ruby(SIDEKIQ_YML_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("sidekiq.yml passed"), "{}", run.stdout);
    let registry = read(&run.emitted, "runtime/job_registry.rb");
    assert!(registry.contains("  def self.sidekiq_worker_queues\n    \"critical default,2\"\n  end\n"), "{registry}");
}

const CABLE_REDIS: &str = "production:\n  adapter: redis\n  url: <%= ENV.fetch(\"REDIS_URL\") { \"redis://localhost:6379/1\" } %>\n  channel_prefix: depot_production\n";

const CABLE_SCRIPT: &str = r##"SENT = []
RedisCable.define_singleton_method(:publish) { |stream, json| SENT << [RedisCable.channel(stream), json] }
raise "not on the Redis cable" unless RedisCable.enabled?
Broadcasts.set_transport(RedisCable::Transport.new)
Turbo::StreamsChannel.broadcast_append_to("Z2lkOi8vZGVwb3QvQ2FyLzE", target: "cars", html: %(<div id="car_1">Golf &amp; "GTI" \u00e9</div>))
ActionCable.server.broadcast("user_7_unreads", { "roomId" => 1, "count" => 2, "note" => "<b>&</b>" })
ActionCable.server.broadcast("user_7_unreads", JSON.generate({ "roomId" => 3 }), coder: nil)
puts "SENT " + JSON.generate(SENT)
"##;

/// Under config/cable.yml's `adapter: redis`, a tree on `:sidekiq` sends
/// each broadcast through Redis on the channel and with the bytes
/// actioncable 8.1.3's Redis adapter publishes (tests/redis_cable/
/// rails.json): a Turbo Stream fragment, a Hash, and a `coder: nil`
/// String. The spinel tree gets the real transport too; a tree on
/// another cable adapter keeps the façade.
#[test]
fn the_redis_cable_publishes_what_rails_does() {
    let run = on_sidekiq().write("config/cable.yml", CABLE_REDIS).run_ruby(CABLE_SCRIPT);
    run.assert_passes();
    let line = run.stdout.lines().find_map(|l| l.strip_prefix("SENT ")).unwrap_or_else(|| panic!("{}", run.stdout));
    let sent: Vec<(String, String)> = serde_json::from_str(line).unwrap();
    let oracle: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/redis_cable/rails.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(oracle["actioncable"], "8.1.3");
    let want: Vec<(String, String)> = oracle["publishes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["channel"].as_str().unwrap().to_string(), p["payload"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(sent, want, "the runtime's publishes differ from Rails'");
    let registry = read(&run.emitted, "runtime/job_registry.rb");
    assert!(
        registry.contains("  def self.cable_redis_url\n    SidekiqPush.setting_or(\"REDIS_URL\", \"redis://localhost:6379/1\")\n  end\n"),
        "{registry}"
    );

    let (spinel, errors) = on_sidekiq().write("config/cable.yml", CABLE_REDIS).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(read(&spinel, "runtime/redis_cable.rb").contains("Redis.pubsub(url: url)"));
    let (plain, _) = on_sidekiq().emit(BuildTarget::Spinel);
    assert!(read(&plain, "runtime/redis_cable.rb").contains("def self.enabled?\n    false\n"));
    let (ruby, _) = on_sidekiq().emit(BuildTarget::Ruby);
    assert!(read(&ruby, "runtime/redis_cable.rb").contains("def self.enabled?\n    false\n"));
}

const CONFIGURE_CLIENT: &str = "Sidekiq.configure_client do |config|\n  config.redis = { url: ENV.fetch(\"JOBS_REDIS_URL\", \"redis://jobs:6379/1\"), password: ENV[\"JOBS_REDIS_PASSWORD\"] }\nend\n";

/// The app's `configure_client` settings reach the registry the push
/// reads them from, on every Ruby-family lane; an app without them gets
/// `""`, so the push falls back to REDIS_PROVIDER, REDIS_URL, localhost.
#[test]
fn configure_client_settings_reach_the_registry() {
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let (emitted, errors) =
            on_sidekiq().write("config/initializers/sidekiq.rb", CONFIGURE_CLIENT).emit(target);
        assert!(errors.is_empty(), "{errors:#?}");
        let registry = read(&emitted, "runtime/job_registry.rb");
        assert!(
            registry.contains(
                "  def self.sidekiq_redis_url\n    SidekiqPush.setting_or(\"JOBS_REDIS_URL\", \"redis://jobs:6379/1\")\n  end\n"
            ),
            "{registry}"
        );
        assert!(
            registry.contains("  def self.sidekiq_redis_password\n    SidekiqPush.setting(\"JOBS_REDIS_PASSWORD\")\n  end\n"),
            "{registry}"
        );
    }
    let (emitted, _) = on_sidekiq().emit(BuildTarget::Ruby);
    let registry = read(&emitted, "runtime/job_registry.rb");
    assert!(registry.contains("  def self.sidekiq_redis_url\n    \"\"\n  end\n"), "{registry}");
}

/// `self.queue_adapter = :sidekiq` in ApplicationJob puts the whole app
/// on Sidekiq, with no config-file setting.
#[test]
fn application_job_on_sidekiq_gets_a_redis_client() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .edit(
            "app/jobs/application_job.rb",
            "class ApplicationJob < ActiveJob::Base\n",
            "class ApplicationJob < ActiveJob::Base\n  self.queue_adapter = :sidekiq\n",
        )
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(read(&emitted, "Gemfile").contains("gem \"redis\""));
    assert!(read(&emitted, "runtime/job_registry.rb").contains("  def self.sidekiq?\n    true\n  end\n"));
}

const PUSH_SCRIPT: &str = r##"ActiveJob.register_drain
SidekiqPush.start_recording
article = Article.create!(title: "Pushed title", body: "A long enough article body.")
article.enqueue_job
got = SidekiqPush.recorded
raise "recorded #{got.inspect}" unless got.length == 2 && got[0] == "sadd queues default"
verb, key, json = got[1].split(" ", 3)
raise "#{verb} #{key}" unless verb == "lpush" && key == "queue:default"
env = JSON.parse(json)
raise "class #{env["class"]}" unless env["class"] == "Sidekiq::ActiveJob::Wrapper"
raise "wrapped #{env["wrapped"]}" unless env["wrapped"] == "ArticleJob" && env["queue"] == "default"
aj = env["args"][0]
gid = "gid://#{Rails.application.global_id_app}/Article/#{article.id}"
raise "arguments #{aj["arguments"]}" unless aj["arguments"] == [{ "_aj_globalid" => gid }]
raise "queue_name #{aj["queue_name"]}" unless aj["queue_name"] == "default" && aj["scheduled_at"].nil?
raise "queued in process too" unless ActiveJob.pending_count == 0
puts "sidekiq push passed"
"##;

/// Under `:sidekiq` the job's `perform_later` pushes its payload in
/// Sidekiq's envelope, recorded here through `SidekiqPush`'s seam (no
/// Redis, no redis gem), and nothing is queued in process.
#[test]
fn perform_later_pushes_the_payload_to_redis() {
    let run = on_sidekiq().run_ruby(PUSH_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("sidekiq push passed"), "{}", run.stdout);
    let job = read(&run.emitted, "app/models/article_job.rb");
    assert!(job.contains("JobRegistry.push_article_job(\"default\", 0.0, "), "{job}");
    let registry = read(&run.emitted, "runtime/job_registry.rb");
    assert!(registry.contains("def self.push_article_job(queue, at, article)"), "{registry}");
}

const COMMIT_JOB: &str = r##"class CommitJob < ApplicationJob
  self.enqueue_after_transaction_commit = true

  def perform(article)
    article.title
  end
end
"##;

const COMMIT_ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_commit_job
    CommitJob.perform_later(self)
  end
"##;

const COMMIT_SCRIPT: &str = r##"ActiveJob.register_drain
SidekiqPush.start_recording
Article.transaction do
  article = Article.create!(title: "Committed title", body: "A long enough article body.")
  article.enqueue_commit_job
  raise "pushed inside the transaction" unless SidekiqPush.recorded.empty?
end
raise "not pushed at commit: #{SidekiqPush.recorded.inspect}" unless SidekiqPush.recorded.length == 2
SidekiqPush.start_recording
begin
  Article.transaction do
    article = Article.create!(title: "Rolled back title", body: "Another long enough body.")
    article.enqueue_commit_job
    raise "roll back"
  end
rescue RuntimeError => e
  raise e unless e.message == "roll back"
end
raise "a rolled-back push went out" unless SidekiqPush.recorded.empty?
puts "after commit push passed"
"##;

/// An after-commit job is pushed at COMMIT and dropped at ROLLBACK, the
/// same hold Phase 1's in-process queue has.
#[test]
fn an_after_commit_job_is_pushed_at_commit() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/commit_job.rb", COMMIT_JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", COMMIT_ENQUEUE)
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .run_ruby(COMMIT_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("after commit push passed"), "{}", run.stdout);
}

const HASH_JOB: &str = r##"class OptionsJob < ApplicationJob
  def perform(options)
    options
  end
end
"##;

const HASH_ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_options_job
    OptionsJob.perform_later({ "title" => title })
  end
"##;

/// A job whose arguments can't be serialized keeps its Proc with a
/// warning on the in-process queue, and is an error under `:sidekiq`,
/// where a Proc can't be pushed to Redis.
#[test]
fn an_unplannable_job_is_an_error_only_under_sidekiq() {
    let overlay = || {
        emit_and_run::real_blog()
            .write("app/jobs/options_job.rb", HASH_JOB)
            .edit("app/models/article.rb", "  validates :title, presence: true\n", HASH_ENQUEUE)
    };
    let (_, errors) = overlay()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit(BuildTarget::Ruby);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].contains("OptionsJob") && errors[0].contains("queue_adapter :sidekiq"), "{errors:#?}");

    let (_, errors) = overlay().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// The targets with no Sidekiq push say so once per app on `:sidekiq`,
/// and only when the app has jobs; the Ruby family says nothing. A
/// strict target's tree is the same with or without the adapter.
#[test]
fn strict_targets_warn_once_and_emit_the_same_tree() {
    use roundhouse::lower::job_payload::sidekiq_adapter_target_warning;
    let (_, app, _) = on_sidekiq().emit_with_app(BuildTarget::Ruby);
    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        assert!(sidekiq_adapter_target_warning(&app, target).is_none(), "{}", target.as_str());
    }
    let strict = [BuildTarget::Go, BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Elixir, BuildTarget::Crystal];
    for target in strict {
        let w = sidekiq_adapter_target_warning(&app, target).expect("a warning");
        assert_eq!(w.severity, roundhouse::diagnostic::Severity::Warning);
        assert!(w.message.contains(target.as_str()), "{}", w.message);
    }

    let (_, no_jobs, _) = emit_and_run::real_blog()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit_with_app(BuildTarget::Ruby);
    assert!(sidekiq_adapter_target_warning(&no_jobs, BuildTarget::Go).is_none());

    let tree = |overlay: emit_and_run::Overlay| {
        let (emitted, errors) = overlay.emit(BuildTarget::Go);
        assert!(errors.is_empty(), "{errors:#?}");
        let mut files = Vec::new();
        let mut stack = vec![emitted.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path.strip_prefix(emitted.as_path()).unwrap().display().to_string();
                    files.push((rel, std::fs::read(&path).unwrap()));
                }
            }
        }
        files.sort();
        files
    };
    let plain = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE);
    let with = tree(on_sidekiq());
    let without = tree(plain);
    assert_eq!(with.len(), without.len(), "file count");
    for ((p1, c1), (p2, c2)) in with.iter().zip(&without) {
        assert_eq!(p1, p2);
        if p1.contains("production") {
            continue;
        }
        assert!(c1 == c2, "{p1} differs with :sidekiq on go");
    }
}

const SET_ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_job
    ArticleJob.perform_later(self)
  end

  def enqueue_variants
    ArticleJob.set(wait: 5.minutes).perform_later(self)
    ArticleJob.set(wait: 90).perform_later(self)
    ArticleJob.set(wait_until: Time.now + 3600).perform_later(self)
    ArticleJob.set(wait_until: Time.now - 60).perform_later(self)
    ArticleJob.set(queue: :low).perform_later(self)
    ArticleJob.set(queue: "critical", wait: 30.seconds).perform_later(self)
    ArticleJob.set(wait: 10, wait_until: Time.now + 7200).perform_later(self)
    ArticleJob.set(wait_until: Time.now + 7200, wait: 10).perform_later(self)
  end
"##;

const SET_SCRIPT: &str = r##"ActiveJob.register_drain
SidekiqPush.start_recording
article = Article.create!(title: "Scheduled title", body: "A long enough article body.")
t0 = Time.now.to_f
article.enqueue_variants
lines = SidekiqPush.recorded
def env(line)
  JSON.parse(line.split(" ", 4).last)
end
zadds = lines.select { |l| l.start_with?("zadd ") }
lpushes = lines.select { |l| l.start_with?("lpush ") }
raise "zadds #{zadds.length} #{lines.inspect}" unless zadds.length == 6
raise "lpushes #{lpushes.length}" unless lpushes.length == 2
score = ->(l) { l.split(" ", 4)[2].to_f }
raise "5.minutes #{score.(zadds[0]) - t0}" unless (score.(zadds[0]) - t0 - 300).abs < 5
raise "90 #{score.(zadds[1]) - t0}" unless (score.(zadds[1]) - t0 - 90).abs < 5
raise "wait_until #{score.(zadds[2]) - t0}" unless (score.(zadds[2]) - t0 - 3600).abs < 5
raise "queue+wait #{score.(zadds[3]) - t0}" unless (score.(zadds[3]) - t0 - 30).abs < 5
raise "queue+wait queue" unless env(zadds[3])["queue"] == "critical" && env(zadds[3])["args"][0]["queue_name"] == "critical"
raise "wait_until wins #{score.(zadds[4]) - t0}" unless (score.(zadds[4]) - t0 - 7200).abs < 5
raise "wait_until wins, either order #{score.(zadds[5]) - t0}" unless (score.(zadds[5]) - t0 - 7200).abs < 5
raise "scheduled_at" unless env(zadds[0])["args"][0]["scheduled_at"].is_a?(String) && !env(zadds[0]).key?("enqueued_at")
due = lpushes[0]
raise "due #{due}" unless due.start_with?("lpush queue:default ") && env(due)["args"][0]["scheduled_at"].is_a?(String)
low = lpushes[1]
raise "low #{low}" unless low.start_with?("lpush queue:low ") && env(low)["args"][0]["queue_name"] == "low"
raise "queues" unless lines.include?("sadd queues low") && lines.include?("sadd queues default")
puts "set options passed"
"##;

/// `set(wait:, wait_until:, queue:)` under `:sidekiq`: a wait or a
/// future time is a ZADD at the right score, a time already past is
/// pushed at once with its `scheduled_at`, and `queue:` sets both the
/// envelope's queue and the ActiveJob `queue_name`. Given both,
/// `wait_until:` wins over `wait:`, as in Rails. The sites are
/// honored, so they file no `job-set-options` warning.
#[test]
fn set_options_schedule_and_route_the_push() {
    let run = on_sidekiq()
        .edit("app/models/article.rb", ENQUEUE, SET_ENQUEUE)
        .run_ruby(SET_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("set options passed"), "{}", run.stdout);
    let job = read(&run.emitted, "app/models/article_job.rb");
    assert!(job.contains("def self.perform_later_with(at, queue, article)"), "{job}");
    assert!(job.contains("JobRegistry.push_article_job(queue, at, article)"), "{job}");
    let model = read(&run.emitted, "app/models/article.rb");
    assert!(model.contains("Time.now.to_f + 300.0"), "{model}");
}

/// A literal Duration `wait:` folds to its seconds, so the strict
/// targets, which have no Duration, emit the same `set(...)` sites
/// cleanly under `:sidekiq`.
#[test]
fn a_literal_duration_wait_emits_on_go() {
    let (_, errors) = on_sidekiq().edit("app/models/article.rb", ENQUEUE, SET_ENQUEUE).emit(BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:#?}");
}

const COLLIDER_JOB: &str = r##"class ColliderJob < ApplicationJob
  queue_as :default

  def perform(queue, at, q)
    queue + at + q
  end
end
"##;

const COLLIDER_ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_job
    ColliderJob.perform_later("mine", "stamp", "own")
    ColliderJob.set(wait: 60).perform_later("mine", "stamp", "own")
  end
"##;

const COLLIDER_SCRIPT: &str = r##"ActiveJob.register_drain
SidekiqPush.start_recording
Article.create!(title: "Collider title", body: "A long enough article body.").enqueue_job
lines = SidekiqPush.recorded
def env(line)
  JSON.parse(line.split(" ", 4).last)
end
lpush = lines.find { |l| l.start_with?("lpush queue:default ") }
zadd = lines.find { |l| l.start_with?("zadd ") }
raise "lines #{lines.inspect}" unless lpush && zadd
[lpush, zadd].each do |l|
  args = env(l)["args"][0]["arguments"]
  raise "arguments #{args.inspect}" unless args == ["mine", "stamp", "own"]
end
puts "collider passed"
"##;

/// A job whose own parameters are named `queue`, `at` and `q`: the push
/// path's added parameters and local take fresh names, so the job's
/// arguments reach the payload unchanged, scheduled or not.
#[test]
fn a_job_whose_parameters_are_named_queue_at_and_q_still_pushes_its_arguments() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/collider_job.rb", COLLIDER_JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", COLLIDER_ENQUEUE)
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .run_ruby(COLLIDER_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("collider passed"), "{}", run.stdout);
    let job = read(&run.emitted, "app/models/collider_job.rb");
    assert!(job.contains("def self.perform_later_with(at_, queue_, queue, at, q)"), "{job}");
    let registry = read(&run.emitted, "runtime/job_registry.rb");
    assert!(registry.contains("def self.push_collider_job(queue_, at_, queue, at, q)"), "{registry}");
    assert!(registry.contains("q_ = queue_ == \"\" ? \"default\" : queue_"), "{registry}");
}

const PRIORITY_ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_job
    ArticleJob.set(priority: 10, queue: :low).perform_later(self)
  end
"##;

const DYNAMIC_ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_job(options)
    ArticleJob.set(options).perform_later(self)
  end
"##;

/// `priority:` is dropped with a warning (Sidekiq ignores it); a
/// `set(...)` that isn't literal options is an error under `:sidekiq`,
/// because the job would run early or on the wrong queue.
#[test]
fn priority_warns_and_a_computed_set_is_an_error() {
    let (_, errors) = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", PRIORITY_ENQUEUE)
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");

    let (_, errors) = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", DYNAMIC_ENQUEUE)
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit(BuildTarget::Ruby);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].contains("can't be honored"), "{errors:#?}");
}

/// A tree on `:sidekiq` documents its worker role under the README's
/// `## Run`, as a `###` part so scripts/smoke doesn't start it, with the
/// cable.yml channel its broadcasts go out on; a tree that isn't on
/// Sidekiq has no such part.
#[test]
fn the_readme_says_how_to_run_the_worker() {
    for (target, worker) in [(BuildTarget::Spinel, "./build/bin/blog sidekiq"), (BuildTarget::Ruby, "bundle exec ruby sidekiq.rb"), (BuildTarget::Jruby, "jruby -S bundle exec jruby sidekiq.rb")] {
        let (emitted, errors) = on_sidekiq().write("config/cable.yml", CABLE_REDIS).emit(target);
        assert!(errors.is_empty(), "{errors:#?}");
        let readme = read(&emitted, "README.md");
        let run = readme.find("## Run\n").unwrap_or_else(|| panic!("{readme}"));
        let jobs = readme.find("### Jobs on Sidekiq\n").unwrap_or_else(|| panic!("{readme}"));
        let next = readme[run + 1..].find("\n## ").map(|i| run + 1 + i).unwrap_or(readme.len());
        assert!(run < jobs && jobs < next, "{readme}");
        assert!(readme[jobs..next].contains(&format!("```sh\n{worker}\n```\n")), "{readme}");
        assert!(readme[jobs..next].contains("`depot_production:<stream>`"), "{readme}");
    }
    let (plain, _) = on_sidekiq().emit(BuildTarget::Ruby);
    assert!(read(&plain, "README.md").contains("reaches no browser"));
    let (off, _) = emit_and_run::real_blog().emit(BuildTarget::Ruby);
    assert!(!read(&off, "README.md").contains("### Jobs on Sidekiq"));
}
