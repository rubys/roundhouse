use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        r#"  validates :title, presence: true

  def self.civil_days
    [
      Time.at(0).getlocal(3600).to_date.iso8601,
      Time.at(0).getlocal(-3600).to_date.iso8601,
      Time.utc(2024, 3, 1, 0, 30).getlocal(-3600).to_date.iso8601,
      Time.utc(2024, 2, 29, 23, 30).getlocal(3600).to_date.iso8601,
      Time.local(2024, 2, 29, 23, 45).to_date.iso8601,
      Time.utc(2024, 2, 29, 16).in_time_zone("Tokyo").to_date.iso8601,
      Time.utc(2024, 3, 1, 0, 30).in_time_zone("America/New_York").to_date.iso8601
    ]
  end
"#,
    )
}

const ASSERTIONS: &str = r#"
expected = ["1970-01-01", "1969-12-31", "2024-02-29", "2024-03-01", "2024-02-29", "2024-03-01", "2024-02-29"]
actual = Article.civil_days
raise "civil days: #{actual.inspect}" unless actual == expected
puts "Time#to_date preserves local and zoned days"
"#;

#[test]
fn time_to_date_runs_in_emitted_ruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn time_to_date_runs_in_emitted_spinel() {
    app().run_spinel_with_rbs(ASSERTIONS).assert_passes();
}
