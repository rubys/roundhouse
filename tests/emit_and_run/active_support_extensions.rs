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

  def self.report_unhandled(error)
    Rails.error.report(error, handled: false)
  end

  def self.report_info(error)
    Rails.error.report(error, handled: true, severity: :info)
  end

  def self.report_debug(error)
    Rails.error.report(error, handled: true, severity: :debug)
  end

  def self.option_labels(options)
    Array(options).pluck(\"label\")
  end

  def self.option_values(options, key)
    Array(options).pluck(key)
  end

  def self.option_values_by(options, __pluck)
    Array(options).pluck(__pluck)
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
fn rails_error_report_logs_at_its_severity_in_emitted_ruby() {
    let run = app().run_ruby(
        r#"raise "report returns nil" unless Article.report(RuntimeError.new("handled")).nil?
Article.report_unhandled(RuntimeError.new("unhandled"))
Article.report_info(RuntimeError.new("info"))
begin
  Article.report_debug(RuntimeError.new("debug"))
  raise "debug severity was accepted"
rescue ArgumentError => e
  raise "message: #{e.message}" unless e.message == "severity must be one of :error, :warning, :info, got: :debug"
end
puts "error report passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("error report passed"));
    for line in ["WARN RuntimeError: handled", "ERROR RuntimeError: unhandled", "INFO RuntimeError: info"] {
        assert!(run.stderr.contains(line), "{line}: {}", run.stderr);
    }
}

#[test]
fn array_pluck_reads_each_hash_in_emitted_ruby() {
    let run = app().run_ruby(
        r#"labels = Article.option_labels([{ "label" => "a" }, { "label" => "b" }, {}])
raise "labels: #{labels.inspect}" unless labels == ["a", "b", nil]
raise "nil options" unless Article.option_labels(nil) == []
raise "local key" unless Article.option_values([{ id: 1 }, { id: 2 }], :id) == [1, 2]
raise "key local named __pluck" unless Article.option_values_by([{ id: 1 }, { id: 2 }], :id) == [1, 2]
puts "array pluck passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("array pluck passed"));
}
