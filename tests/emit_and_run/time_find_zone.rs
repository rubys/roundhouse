use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        r#"  validates :title, presence: true

  def self.find_zone_name(name)
    Time.find_zone(name)&.name
  end

  def self.find_zone_offset(name)
    Time.find_zone(name)&.utc_offset
  end

  def self.find_zone_fallback(name)
    zone = Time.find_zone(name) || Time.find_zone("UTC")
    Time.at(0).in_time_zone(zone).strftime("%Y-%m-%d %H:%M %z")
  end
"#,
    )
}

const ASSERTIONS: &str = r#"
raise "UTC" unless Article.find_zone_name("UTC") == "UTC"
raise "Rails name" unless Article.find_zone_name("Tokyo") == "Tokyo"
raise "IANA name" unless Article.find_zone_name("Asia/Tokyo") == "Asia/Tokyo"
raise "invalid" unless Article.find_zone_name("NOT-A-TIMEZONE").nil?
raise "empty" unless Article.find_zone_name("").nil?
raise "nil" unless Article.find_zone_name(nil).nil?
raise "offset" unless Article.find_zone_offset("Tokyo") == 32400
raise "invalid offset" unless Article.find_zone_offset("Nowhere").nil?
raise "fallback" unless Article.find_zone_fallback("Nowhere") == "1970-01-01 00:00 +0000"
raise "local" unless Article.find_zone_fallback("Tokyo") == "1970-01-01 09:00 +0900"
puts "Time.find_zone checks passed"
"#;

#[test]
fn time_find_zone_runs_in_emitted_ruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn time_find_zone_runs_in_emitted_spinel() {
    app().run_spinel_with_rbs(ASSERTIONS).assert_passes();
}
