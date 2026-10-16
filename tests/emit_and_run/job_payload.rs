//! An ActiveJob payload on the CRuby lane: a job whose arguments are a
//! record, a String, an Integer, a Symbol, a Time and an Array goes
//! through the drain as a serialized payload, gets equal values back in
//! `perform` (the record looked up again), and a record deleted before
//! the drain runs is a DeserializationError that `discard_on` drops.

use super::emit_and_run;

const JOB: &str = r#"class ArticleJob < ApplicationJob
  discard_on ActiveJob::DeserializationError

  def perform(article, label, count, kind, at, ids)
    puts "performed #{article.title} #{label} #{count} #{kind} #{at.utc.iso8601(9)} #{ids.inspect}"
  end
end
"#;

const ENQUEUE: &str = r#"  validates :title, presence: true

  def enqueue_job
    ArticleJob.perform_later(self, "a \"label\"", 3, :fresh, Time.at(1_791_548_096, 5, :nsec).utc, [1, 2])
  end
"#;

const SCRIPT: &str = r#"ActiveJob.register_drain
article = Article.create!(title: "Queued title", body: "A long enough article body.")
article.enqueue_job
raise "nothing queued" unless ActiveJob.pending_count == 1
# The payload rides the queue inside a Proc; read the same text the
# enqueue wrote from the registry's writer.
payload = JSON.parse(JobRegistry.payload_article_job(article, "a \"label\"", 3, :fresh, Time.at(1_791_548_096, 5, :nsec).utc, [1, 2]))
raise "job_class #{payload["job_class"]}" unless payload["job_class"] == "ArticleJob"
raise "queue #{payload["queue_name"]}" unless payload["queue_name"] == "default"
gid = "gid://#{Rails.application.global_id_app}/Article/#{article.id}"
raise "gid #{payload["arguments"][0]}" unless payload["arguments"][0] == { "_aj_globalid" => gid }
# The job sees the committed row, not the object it was enqueued with.
Article.find(article.id).update!(title: "Changed title")
raise "drain ran #{ActiveJob.pending_count}" unless ActiveJob.drain == 1

# A record deleted before the drain: discarded, not run.
gone = Article.create!(title: "Gone title", body: "Another long enough body.")
gone.enqueue_job
gone.destroy
# Its Proc runs; the job inside it is discarded, not performed.
ActiveJob.drain
puts "payload round trip passed"
"#;

#[test]
fn a_payload_job_round_trips_through_the_drain() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .run_ruby(SCRIPT);
    run.assert_passes();
    assert!(
        run.stdout.contains(
            r#"performed Changed title a "label" 3 fresh 2026-10-09T12:14:56.000000005Z [1, 2]"#
        ),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("payload round trip passed"), "{}", run.stdout);
    assert!(!run.stdout.contains("performed Gone title"), "{}", run.stdout);
    assert!(
        !run.stderr.contains("ArticleJob: Error"),
        "a discard_on job is dropped quietly:\n{}",
        run.stderr
    );
    let job = std::fs::read_to_string(run.emitted.join("app/models/article_job.rb"))
        .expect("emitted job");
    assert!(job.contains("ActiveJob.enqueue_payload(JobRegistry.payload_article_job("), "{job}");
}

const AFTER_COMMIT_JOB: &str = r#"class CommitJob < ApplicationJob
  self.enqueue_after_transaction_commit = true

  def perform(article)
    puts "committed job saw #{article.title}"
  end
end
"#;

const AFTER_COMMIT_ENQUEUE: &str = r#"  validates :title, presence: true

  def enqueue_commit_job
    CommitJob.perform_later(self)
  end
"#;

const AFTER_COMMIT_SCRIPT: &str = r#"ActiveJob.register_drain
Article.transaction do
  article = Article.create!(title: "Committed title", body: "A long enough article body.")
  article.enqueue_commit_job
  raise "queued inside the transaction" unless ActiveJob.pending_count == 0
end
raise "not queued at commit" unless ActiveJob.pending_count == 1
raise "drain ran #{ActiveJob.pending_count}" unless ActiveJob.drain == 1

begin
  Article.transaction do
    article = Article.create!(title: "Rolled back title", body: "Another long enough body.")
    article.enqueue_commit_job
    raise "roll back"
  end
rescue RuntimeError => e
  raise e unless e.message == "roll back"
end
raise "a rolled-back enqueue was queued" unless ActiveJob.pending_count == 0

article = Article.create!(title: "Outside title", body: "A third long enough body.")
article.enqueue_commit_job
raise "outside a transaction it queues at once" unless ActiveJob.pending_count == 1
ActiveJob.drain
puts "after commit passed"
"#;

/// `self.enqueue_after_transaction_commit = true` (Lobsters'
/// `ApplicationJob`): a payload enqueued inside `Model.transaction` is
/// queued at COMMIT, so the drain never looks for a row before it is
/// visible, and one enqueued in a transaction that rolls back never runs.
#[test]
fn an_after_commit_job_waits_for_the_commit() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/commit_job.rb", AFTER_COMMIT_JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", AFTER_COMMIT_ENQUEUE)
        .run_ruby(AFTER_COMMIT_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("committed job saw Committed title"), "{}", run.stdout);
    assert!(!run.stdout.contains("Rolled back title"), "{}", run.stdout);
    assert!(run.stdout.contains("after commit passed"), "{}", run.stdout);
    let job = std::fs::read_to_string(run.emitted.join("app/models/commit_job.rb"))
        .expect("emitted job");
    assert!(
        job.contains("ActiveJob.enqueue_payload_after_commit(JobRegistry.payload_commit_job(article))"),
        "{job}"
    );
}

const STI_SCRIPT: &str = r#"ActiveJob.register_drain
featured = Articles::Featured.create!(title: "Featured title", body: "A long enough article body.")
row = Article.find(featured.id)
row.enqueue_job
payload = JSON.parse(JobRegistry.payload_article_job(row, "x", 1, :a, Time.now.utc, []))
gid = "gid://#{Rails.application.global_id_app}/Articles::Featured/#{featured.id}"
raise "gid #{payload["arguments"][0]}" unless payload["arguments"][0] == { "_aj_globalid" => gid }
raise "drain ran #{ActiveJob.pending_count}" unless ActiveJob.drain == 1
puts "sti payload passed"
"#;

/// An STI row writes its subclass's name, as Rails does, however it was
/// loaded (rows hydrate base-classed here, so the name comes from the
/// `type` column), and the registry's locator accepts that name and
/// finds the row on the base.
#[test]
fn an_sti_record_round_trips_under_its_subclass_name() {
    let run = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.string \"title\"\n    t.text \"body\"\n",
            "    t.string \"title\"\n    t.string \"type\"\n    t.text \"body\"\n",
        )
        .write("app/models/articles/featured.rb", "class Articles::Featured < Article\nend\n")
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .run_ruby(STI_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("performed Featured title"), "{}", run.stdout);
    assert!(run.stdout.contains("sti payload passed"), "{}", run.stdout);
}
