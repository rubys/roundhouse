//! `sql.active_record` and the render events on the Spinel tree: the
//! SQLite shim times each statement and publishes it when done, a
//! query-cache hit as `cached: true`, and a lowered partial publishes
//! render_partial with its `app/views/…` identifier. Needs a Spinel
//! compiler, so `#[ignore]`d like its sibling suites (CI's
//! `spinel-framework` job runs it, `scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SCRIPT: &str = r##"
sqls = [""]
sqls.pop
cached = [false]
cached.pop
spans = [0.0]
spans.pop
ActiveSupport::Notifications.monotonic_subscribe("sql.active_record") do |_name, start, finish, _id, payload|
  sqls << payload[:sql].to_s
  spans << (finish - start)
  cached << (payload[:cached] == true)
end
rendered = [""]
rendered.pop
ActiveSupport::Notifications.monotonic_subscribe("render_partial.action_view") do |name, _start, _finish, _id, payload|
  rendered << name + " " + payload[:identifier].to_s
end
article = Article.new(title: "Hello", body: "A body that is long enough")
article.save!
Db.with_connection do
  Article.all.to_a
  Article.all.to_a
  Views::Articles.article(article)
end
raise "no select: #{sqls}" unless sqls.any? { |s| s.include?("FROM articles") }
raise "no cache hit: #{cached}" unless cached.include?(true)
raise "a negative span: #{spans}" if spans.any? { |s| s < 0.0 }
raise "every span is zero: #{spans}" unless spans.any? { |s| s > 0.0 }
raise "partial: #{rendered}" unless rendered.include?("render_partial.action_view app/views/articles/_article.html.erb")
puts "ok"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn statements_and_partials_publish_on_spinel() {
    let run = emit_and_run::real_blog().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("ok"), "{}", run.stdout);
}
