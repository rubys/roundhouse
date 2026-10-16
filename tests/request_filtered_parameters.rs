//! `request.filtered_parameters`, filtered with the app's
//! `config.filter_parameters`.
//!
//! A controller that logs a request's parameters through `Rails.logger`
//! has to filter them itself, and `request.filtered_parameters` is the
//! documented way (`Rails.logger` does not apply `filter_parameters`;
//! only ActionDispatch's own request log does). The runtime Request had
//! no such reader, so every action that logged its parameters raised
//! NoMethodError before doing its work, and the initializer that lists
//! the filters was recorded as having no effect.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn filtered_parameters_masks_the_configured_keys() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Shop\n  class Application < Rails::Application\n    config.load_defaults 8.1\n    config.api_only = true\n  end\nend\n",
        )
        .write(
            "config/initializers/filter_parameter_logging.rb",
            "Rails.application.config.filter_parameters += [\n  :passw, \"name\"\n]\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  def filtered
    render json: { logged: request.filtered_parameters }
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  post \"widgets/filtered\", to: \"widgets#filtered\"\nend\n",
        )
        .run_ruby(
            r##"
body = %({"widget":{"name":"delta","size":"3","password_confirmation":"x"}})
env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/widgets/filtered", "QUERY_STRING" => "", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s, "rack.input" => StringIO.new(body) }
status, _headers, out = Main.run_rack(env)
text = +""
out.each { |part| text << part }
want = %({"logged":{"widget":{"name":"[FILTERED]","size":"3","password_confirmation":"[FILTERED]"},"controller":"widgets","action":"filtered"}})
raise "#{status} #{text}" unless status == 200 && text == want
puts "ALL OK"
"##,
        )
        .assert_passes();
}

/// The `"controller"` and `"action"` of `path_parameters` are the ones the
/// request was dispatched to, as Rails' are (Rails dispatches by them), even
/// when a route capture has the same name. A later plain
/// `config.filter_parameters = [...]` replaces the names listed before it.
#[test]
fn dispatched_controller_and_action_win_and_a_plain_assignment_replaces() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Shop\n  class Application < Rails::Application\n    config.load_defaults 8.1\n    config.api_only = true\n    config.filter_parameters += [:size]\n  end\nend\n",
        )
        .write(
            "config/initializers/filter_parameter_logging.rb",
            "Rails.application.config.filter_parameters = [:name]\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def filtered\n    render json: { logged: request.filtered_parameters }\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  post \"widgets/filtered/:action\", to: \"widgets#filtered\"\nend\n",
        )
        .run_ruby(
            r##"
body = %({"widget":{"name":"delta","size":"3"}})
env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/widgets/filtered/other", "QUERY_STRING" => "", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s, "rack.input" => StringIO.new(body) }
status, _headers, out = Main.run_rack(env)
text = +""
out.each { |part| text << part }
want = %({"logged":{"action":"filtered","widget":{"name":"[FILTERED]","size":"3"},"controller":"widgets"}})
raise "#{status} #{text}" unless status == 200 && text == want
puts "ALL OK"
"##,
        )
        .assert_passes();
}

/// A computed `config.filter_parameters = ...` is not read, but it still
/// replaces the names listed before it, as a literal `=` does.
#[test]
fn a_computed_assignment_still_discards_the_earlier_names() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Shop\n  class Application < Rails::Application\n    config.load_defaults 8.1\n    config.api_only = true\n    config.filter_parameters += [:size]\n  end\nend\n",
        )
        .write(
            "config/initializers/filter_parameter_logging.rb",
            "Rails.application.config.filter_parameters = ENV.fetch(\"FILTER_KEYS\", \"\").split(\",\")\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def filtered\n    render json: { logged: request.filtered_parameters }\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  post \"widgets/filtered\", to: \"widgets#filtered\"\nend\n",
        )
        .run_ruby(
            r##"
body = %({"widget":{"name":"delta","size":"3"}})
env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/widgets/filtered", "QUERY_STRING" => "", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s, "rack.input" => StringIO.new(body) }
status, _headers, out = Main.run_rack(env)
text = +""
out.each { |part| text << part }
raise "#{status} #{text}" unless status == 200 && !text.include?("[FILTERED]") && text.include?(%("size":"3"))
puts "ALL OK"
"##,
        )
        .assert_passes();
}
