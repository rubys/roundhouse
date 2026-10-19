//! `queue_adapter :sidekiq` on the Ruby-family lanes: what an app that
//! sets it, and has a payload job, gets in its emitted tree. A real
//! Redis client in `runtime/sidekiq_redis.rb` (spinel-redis on spinel,
//! redis-rb on ruby and jruby), the package or gem that provides it,
//! and `JobRegistry.sidekiq?` true. Every other app keeps the raising
//! façade and no dependency.

use roundhouse::project::BuildTarget;

use super::emit_and_run;

const JOB: &str = r#"class ArticleJob < ApplicationJob
  queue_as :default

  def perform(article)
    article.title
  end
end
"#;

const ENQUEUE: &str = r#"  validates :title, presence: true

  def enqueue_job
    ArticleJob.perform_later(self)
  end
"#;

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
    assert!(client.contains("require \"redis\"") && client.contains("Redis.new(url: url)"), "{client}");
    assert!(!emitted.join("runtime/sidekiq_redis.rbs").exists(), "the façade's sidecar must drop");
    let manifest = read(&emitted, "spin.toml");
    assert!(
        manifest.contains(
            "redis = { git = \"https://github.com/rubys/spinel-redis\", ref = \"v0.2.0\" }"
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
