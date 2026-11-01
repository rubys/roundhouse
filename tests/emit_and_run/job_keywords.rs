//! An ActiveJob whose `perform` takes keyword arguments
//! (rubys/roundhouse#1461): the job gets `perform_later` and
//! `perform_now` with `perform`'s own parameter list, each forwarding
//! its keywords by name, and its payload carries them as ActiveJob
//! does, in one trailing Hash marked `_aj_ruby2_keywords` (activejob
//! 8.1.3). An optional keyword the caller leaves at its default is not
//! written, as Rails doesn't write a keyword the caller omits, and reads
//! back as that default.

use super::emit_and_run;

const JOB: &str = r#"class GadgetJob < ApplicationJob
  discard_on ActiveJob::DeserializationError

  def perform(article, count:, label: "touched")
    puts "gadget #{article.title} #{count} #{label}"
  end
end
"#;

const NAMESPACED_JOB: &str = r#"module Gadgets
  class SyncJob < ApplicationJob
    def perform(article_id:, note: nil)
      puts "sync #{Article.find(article_id).title} #{note.inspect}"
    end
  end
end
"#;

const ENQUEUE: &str = r#"  validates :title, presence: true

  def enqueue_gadget
    GadgetJob.perform_later(self, count: 2, label: "custom")
  end

  def enqueue_gadget_default
    GadgetJob.perform_later(self, count: 3)
  end

  def run_gadget_now
    GadgetJob.perform_now(self, count: 4)
  end

  def enqueue_sync
    Gadgets::SyncJob.perform_later(article_id: id, note: "synced")
  end

  def enqueue_sync_default
    Gadgets::SyncJob.perform_later(article_id: id)
  end
"#;

const SCRIPT: &str = r#"ActiveJob.register_drain
article = Article.create!(title: "Keyword title", body: "A long enough article body.")
gid = "gid://#{Rails.application.global_id_app}/Article/#{article.id}"

# The payload: the record first, then the keywords in one Hash.
payload = JSON.parse(JobRegistry.payload_gadget_job(article, 2, "custom"))
args = payload["arguments"]
expected = [
  { "_aj_globalid" => gid },
  { "count" => 2, "label" => "custom", "_aj_ruby2_keywords" => ["count", "label"] }
]
raise "arguments #{args.inspect}" unless args == expected
# An omitted optional keyword is not written.
args = JSON.parse(JobRegistry.payload_gadget_job(article, 3, "touched"))["arguments"]
raise "default arguments #{args.inspect}" unless args[1] == { "count" => 3, "_aj_ruby2_keywords" => ["count"] }
args = JSON.parse(JobRegistry.payload_gadgets__sync_job(article.id, nil))["arguments"]
raise "sync arguments #{args.inspect}" unless args == [{ "article_id" => article.id, "_aj_ruby2_keywords" => ["article_id"] }]

article.enqueue_gadget
article.enqueue_gadget_default
article.enqueue_sync
article.enqueue_sync_default
raise "queued #{ActiveJob.pending_count}" unless ActiveJob.pending_count == 4
raise "drain ran #{ActiveJob.pending_count}" unless ActiveJob.drain == 4
article.run_gadget_now

# A record deleted before the drain is still a DeserializationError.
gone = Article.create!(title: "Gone title", body: "Another long enough body.")
gone.enqueue_gadget
gone.destroy
ActiveJob.drain
puts "keyword jobs passed"
"#;

#[test]
fn a_keyword_job_round_trips_through_the_drain() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/gadget_job.rb", JOB)
        .write("app/jobs/gadgets/sync_job.rb", NAMESPACED_JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .run_ruby(SCRIPT);
    run.assert_passes();
    for line in [
        "gadget Keyword title 2 custom",
        "gadget Keyword title 3 touched",
        "gadget Keyword title 4 touched",
        "sync Keyword title \"synced\"",
        "sync Keyword title nil",
        "keyword jobs passed",
    ] {
        assert!(run.stdout.contains(line), "missing `{line}`:\n{}", run.stdout);
    }
    assert!(!run.stdout.contains("Gone title"), "{}", run.stdout);

    let job = std::fs::read_to_string(run.emitted.join("app/models/gadget_job.rb")).expect("emitted job");
    assert!(job.contains("def self.perform_later(article, count:, label: \"touched\")"), "{job}");
    assert!(job.contains("JobRegistry.payload_gadget_job(article, count, label)"), "{job}");
}

const SIDEKIQ: &str = "  config.active_job.queue_adapter = :sidekiq\n";

/// Under `queue_adapter :sidekiq` a keyword job is pushed as a payload,
/// not refused: the ruby and spinel trees emit with no errors, and the
/// push writes the keywords the way the payload writer does.
#[test]
fn a_keyword_job_on_sidekiq_is_pushed_as_a_payload() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (emitted, errors) = emit_and_run::real_blog()
            .write("app/jobs/gadget_job.rb", JOB)
            .write("app/jobs/gadgets/sync_job.rb", NAMESPACED_JOB)
            .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
            .edit(
                "config/environments/production.rb",
                "  config.active_job.queue_adapter = :solid_queue\n",
                SIDEKIQ,
            )
            .emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
        let registry = std::fs::read_to_string(emitted.join("runtime/job_registry.rb")).expect("registry");
        assert!(
            registry.contains("  def self.push_gadget_job(queue, at, article, count, label)\n"),
            "{registry}"
        );
        assert!(
            registry.contains(
                "ActiveJob::Arguments.keywords([\"count\", (label == \"touched\" ? \"\" : \"label\")], \
                 [ActiveJob::Arguments.int(count), (label == \"touched\" ? \"\" : ActiveJob::Arguments.str(label))])"
            ),
            "{registry}"
        );
        assert!(
            registry.contains(
                "      kw = ActiveJob::Arguments.keywords_at(args, 1, [\"count\", \"label\"])\n      \
                 GadgetJob.new.perform(locate_article_at(args, 0), count: ActiveJob::Arguments.int_at(kw, 0), \
                 label: (ActiveJob::Arguments.null_at(kw, 1) ? \"touched\" : ActiveJob::Arguments.str_at(kw, 1)))\n"
            ),
            "{registry}"
        );
        let job = std::fs::read_to_string(emitted.join("app/models/gadget_job.rb")).expect("emitted job");
        assert!(job.contains("JobRegistry.push_gadget_job(\"default\", 0.0, article, count, label)"), "{job}");
    }
}
