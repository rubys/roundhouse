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
