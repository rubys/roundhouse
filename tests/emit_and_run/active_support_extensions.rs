use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def created_at_in(zone_name)
    created_at.in_time_zone(ActiveSupport::TimeZone[zone_name])
  end

  def self.zone_name_for(name)
    ActiveSupport::TimeZone[name]&.name
  end

  def self.report(error)
    Rails.error.report(error, handled: true)
  end

  def self.option_labels(options)
    Array(options).pluck(\"label\")
  end
",
    )
}

#[test]
fn time_zone_lookup_runs_in_emitted_ruby() {
    let run = app().run_ruby(
        r#"article = Article.create!(title: "Zoned", body: "A sufficiently long body.")
local = article.created_at_in("Tokyo")
raise "offset: #{local.utc_offset}" unless local.utc_offset == 9 * 3600
raise "instant: #{local.inspect}" unless local.to_i == article.created_at.to_i
raise "known zone" unless Article.zone_name_for("Tokyo") == "Tokyo"
raise "IANA zone" unless Article.zone_name_for("Asia/Tokyo") == "Asia/Tokyo"
raise "unknown zone" unless Article.zone_name_for("Nowhere").nil?
puts "time zone lookup passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("time zone lookup passed"));
}

#[test]
fn rails_error_report_logs_in_emitted_ruby() {
    let run = app().run_ruby(
        r#"raise "report returns nil" unless Article.report(RuntimeError.new("boom")).nil?
puts "error report passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("error report passed"));
    assert!(run.stderr.contains("ERROR RuntimeError: boom"), "{}", run.stderr);
}

#[test]
fn array_pluck_reads_each_hash_in_emitted_ruby() {
    let run = app().run_ruby(
        r#"labels = Article.option_labels([{ "label" => "a" }, { "label" => "b" }, {}])
raise "labels: #{labels.inspect}" unless labels == ["a", "b", nil]
raise "nil options" unless Article.option_labels(nil) == []
puts "array pluck passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("array pluck passed"));
}
