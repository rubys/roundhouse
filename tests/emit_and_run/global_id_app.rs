//! `GlobalID.app` for a multi-word application name on the emitted
//! CRuby tree: the dasherized railtie name, as globalid's railtie sets
//! it, so a gid a real Rails process minted for the app is one this
//! runtime reads as its own.

use super::emit_and_run;

#[test]
fn a_multi_word_app_reads_the_gids_rails_mints() {
    emit_and_run::empty_app()
        .write(
            "config/application.rb",
            "module CwtRails\nclass Application < Rails::Application\nend\nend\n",
        )
        .run_ruby(
            r#"
app = Rails.application.global_id_app
raise app.inspect unless app == "cwt-rails"
parts = GlobalID::Locator.parts_from_uri("gid://cwt-rails/Article/7")
raise parts.inspect unless parts == ["cwt-rails", "Article", "7"]
raise "underscored accepted" unless GlobalID::Locator.parts_from_uri("gid://cwt_rails/Article/7").nil?
puts "global id app passed"
"#,
        )
        .assert_passes();
}
