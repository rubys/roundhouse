use super::emit_and_run;

#[test]
fn the_default_key_uses_the_application_name() {
    emit_and_run::empty_app()
        .write("config/application.rb", "module CwtRails\nclass Application < Rails::Application\nend\nend\n")
        .run_ruby("raise Rails.application.session_cookie_key.inspect unless Rails.application.session_cookie_key == \"_cwt_rails_session\"\nputs \"application cookie key passed\"\n")
        .assert_passes();
}

#[test]
fn an_environment_declaration_replaces_the_application_key() {
    emit_and_run::empty_app()
        .write("config/application.rb", "module Demo\nclass Application < Rails::Application\nconfig.session_store :cookie_store, key: \"_application\"\nend\nend\n")
        .write("config/environments/production.rb", "Rails.application.configure do\nconfig.session_store :cookie_store, key: \"_production\"\nend\n")
        .run_ruby(r#"
Rails.env_name = "development"
raise "application" unless Rails.application.session_cookie_key == "_application"
Rails.env_name = "production"
raise "environment" unless Rails.application.session_cookie_key == "_production"
puts "environment cookie key passed"
"#)
        .assert_passes();
}

#[test]
fn declarations_replay_application_environment_and_initializers_in_order() {
    emit_and_run::empty_app()
        .write("config/application.rb", "module Demo\nclass Application < Rails::Application\nconfig.session_store :cookie_store, key: \"_application\"\nend\nend\n")
        .write("config/environments/production.rb", "Rails.application.configure do\nconfig.session_store :cookie_store, key: \"_production\"\nend\n")
        .write("config/initializers/a_cookie.rb", "if Rails.env.test?\nRails.application.config.session_store :cookie_store, key: \"_initializer\"\nend\n")
        .write("config/initializers/z_cookie.rb", "unless Rails.env.production?\nRails.application.config.session_store :cookie_store, key: \"_last\"\nelse\nRails.application.config.session_store :cookie_store, key: \"_production_last\"\nend\n")
        .run_ruby(r#"
Rails.env_name = "test"
raise "initializer order" unless Rails.application.session_cookie_key == "_last"
Rails.env_name = "production"
raise "production" unless Rails.application.session_cookie_key == "_production_last"
Rails.env_name = "development"
raise "conditional initializer" unless Rails.application.session_cookie_key == "_last"
puts "configured cookie keys passed"
"#)
        .assert_passes();
}
