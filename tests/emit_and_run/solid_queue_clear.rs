//! solid_queue's queue API over the in-process queue: a job carries the
//! queue it was put on (`queue_as`, or `set(queue:)` under solid_queue),
//! `SolidQueue::Queue.find_by_name(name).clear` drops that queue's waiting
//! jobs and answers how many, and `SolidQueue::Semaphore` holds no
//! concurrency key. lobsters' RefillStoryPageCacheJob clears its queue and
//! semaphores this way before refilling; both were unsupported constants.
//! Under an adapter other than solid_queue, solid_queue's tables hold no
//! job, so a clear drops nothing.

use super::emit_and_run;

const JOB: &str = r#"class LabelJob < ApplicationJob
  queue_as :alpha

  def perform(label)
    puts "performed #{label}"
  end
end
"#;

const ENQUEUE: &str = r#"  validates :title, presence: true

  def self.enqueue_labels
    LabelJob.perform_later("a1")
    LabelJob.set(queue: :beta).perform_later("b1")
    LabelJob.set(queue: :beta).perform_later("b2")
    LabelJob.perform_later("a2")
  end
"#;

const ADAPTER: &str = "    config.load_defaults 8.1\n    config.active_job.queue_adapter = :solid_queue\n";

const SCRIPT: &str = r#"ActiveJob.register_drain
Article.enqueue_labels
raise "queued #{ActiveJob.pending_count}" unless ActiveJob.pending_count == 4
raise "cleared beta" unless SolidQueue::Queue.find_by_name(:beta).clear == 2
raise "cleared nothing" unless SolidQueue::Queue.find_by_name("gamma").clear == 0
raise "semaphores" unless SolidQueue::Semaphore.where("key LIKE 'LabelJob/%'").delete_all == 0
raise "left #{ActiveJob.pending_count}" unless ActiveJob.pending_count == 2
ActiveJob.drain
puts "solid queue clear passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/jobs/label_job.rb", JOB)
}

#[test]
fn a_queue_clear_drops_that_queues_waiting_jobs() {
    let run = app()
        .edit("config/application.rb", "    config.load_defaults 8.1\n", ADAPTER)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("performed a1\nperformed a2\n"), "{}", run.stdout);
    assert!(!run.stdout.contains("performed b"), "{}", run.stdout);
    assert!(run.stdout.contains("solid queue clear passed"), "{}", run.stdout);
}

#[test]
fn under_another_adapter_a_clear_drops_nothing() {
    let run = app()
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .run_ruby(
            r#"ActiveJob.register_drain
Article.enqueue_labels
raise "cleared" unless SolidQueue::Queue.find_by_name("beta").clear == 0
raise "left #{ActiveJob.pending_count}" unless ActiveJob.pending_count == 4
puts "other adapter passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("other adapter passed"), "{}", run.stdout);
}

#[test]
fn the_constants_check_clean() {
    let errors = app()
        .edit("config/application.rb", "    config.load_defaults 8.1\n", ADAPTER)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .write("app/jobs/refill_job.rb", "class RefillJob < ApplicationJob\n  def perform\n    SolidQueue::Queue.find_by_name(\"alpha\").clear\n    SolidQueue::Semaphore.where(\"key LIKE 'LabelJob/%'\").delete_all\n  end\nend\n")
        .check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}
