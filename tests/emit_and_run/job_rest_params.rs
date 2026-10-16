//! `Job.perform_later(x)` runs `Job#perform(x)`, so what callers enqueue
//! types `perform`'s parameters, and a `*rest` parameter is the Array of
//! what its varargs were observed as (lobsters' `perform(*messages)`).

use super::emit_and_run;

fn job_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/jobs/title_job.rb",
            "class TitleJob < ApplicationJob\n  def perform(*articles)\n    articles.map do |article|\n      shout(article)\n    end\n  end\n\n  def shout(article)\n    article.title.upcase\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.lengths(*articles)\n    articles.map { |a| a.title.length }\n  end\n\n  def announce_self\n    TitleJob.perform_later(self)\n  end\n",
        )
        .write(
            "test/models/title_job_test.rb",
            r#"require "test_helper"

class TitleJobTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  test "an enqueued article reaches perform typed" do
    article = Article.create!(title: "Rest params", body: "Body text here")
    perform_enqueued_jobs do
      article.announce_self
    end
    other = Article.create!(title: "Two", body: "Body text here")
    assert_equal ["REST PARAMS", "TWO"], TitleJob.perform_now(article, other)
    assert_equal [11, 3], Article.lengths(article, other)
  end
end
"#,
        )
}

#[test]
fn a_job_s_rest_parameter_takes_what_callers_enqueue() {
    job_app().run_test("test/models/title_job_test.rb").assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_job_s_rest_parameter_takes_what_callers_enqueue_on_spinel() {
    job_app().run_spinel("article = Article.new(title: \"Rest params\")\nother = Article.new(title: \"Two\")\ngot = [TitleJob.perform_now(article, other), Article.lengths(article, other)]\nraise got.inspect unless got == [[\"REST PARAMS\", \"TWO\"], [11, 3]]\nputs \"rest params passed\"\n").assert_passes();
}
