use super::emit_and_run;

const SITE_SETTINGS: &str = r#"module SiteSettings
  def self.tagline
    "Fresh bread daily"
  end
end
"#;

#[test]
fn a_constant_from_a_file_config_application_requires_runs_in_emitted_ruby() {
    emit_and_run::real_blog()
        .edit(
            "config/application.rb",
            "require \"rails/all\"\n",
            "require \"rails/all\"\n\nrequire_relative \"site_settings\"\n",
        )
        .write("config/site_settings.rb", SITE_SETTINGS)
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def tagline\n    SiteSettings.tagline\n  end\n",
        )
        .run_ruby(
            r##"raise "Article#tagline answered #{Article.new.tagline.inspect}" unless Article.new.tagline == "Fresh bread daily""##,
        )
        .assert_passes();
}
