//! The native half of `emit_and_run::solid_queue_clear`: the queue names
//! ride the spinel drain's queue (`runtime/spinel/thread_state.rb`), and a
//! clear drops one queue's waiting jobs under its lock.

use super::emit_and_run;

const JOB: &str = "class LabelJob < ApplicationJob\n  queue_as :alpha\n\n  def perform(label)\n    puts \"performed #{label}\"\n  end\nend\n";

const ENQUEUE: &str = "  validates :title, presence: true\n\n  def self.enqueue_labels\n    LabelJob.perform_later(\"a1\")\n    LabelJob.set(queue: :beta).perform_later(\"b1\")\n    LabelJob.set(queue: :beta).perform_later(\"b2\")\n    LabelJob.perform_later(\"a2\")\n  end\n";

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_queue_clear_drops_that_queues_waiting_jobs_natively() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/label_job.rb", JOB)
        .edit("config/application.rb", "    config.load_defaults 8.1\n", "    config.load_defaults 8.1\n    config.active_job.queue_adapter = :solid_queue\n")
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .run_spinel_with_db(
            r#"ActiveJob.register_drain
Article.enqueue_labels
raise "queued #{ActiveJob.pending_count}" unless ActiveJob.pending_count == 4
raise "cleared beta" unless SolidQueue::Queue.find_by_name(:beta).clear == 2
raise "semaphores" unless SolidQueue::Semaphore.where("key LIKE 'LabelJob/%'").delete_all == 0
raise "left #{ActiveJob.pending_count}" unless ActiveJob.pending_count == 2
ActiveJob.drain
puts "solid queue clear passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("performed a1\nperformed a2\n"), "{}", run.stdout);
    assert!(!run.stdout.contains("performed b"), "{}", run.stdout);
    assert!(run.stdout.contains("solid queue clear passed"), "{}", run.stdout);
}
