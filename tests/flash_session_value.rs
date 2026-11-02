#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn flash_session_value_contains_only_messages_kept_for_the_next_request() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/message\", to: \"messages#show\"\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/messages_controller.rb", r#"class MessagesController < ApplicationController
  def show
    flash[:notice] = "saved"
    kept = flash.to_session_value&.fetch("flashes") || {}
    render plain: kept.fetch("notice", "")
  end
end
"#)
        .run_ruby(r#"
out = StringIO.new
Main.run({"REQUEST_METHOD" => "GET", "PATH_INFO" => "/message", "HTTP_HOST" => "example.test"}, StringIO.new, out)
raise out.string unless out.string.end_with?("saved")
flash = ActionDispatch::Flash.new
raise "empty flash retained" unless flash.to_session_value.nil?
flash[:notice] = "saved"
raise "session value differs" unless flash.to_session_value == {"discard" => [], "flashes" => {"notice" => "saved"}}
flash.now[:alert] = "only now"
raise "flash.now retained" if flash.to_session_value["flashes"].key?("alert")
next_flash = ActionDispatch::Flash.new(flash.to_persisted)
raise "consumed flash retained" unless next_flash.to_session_value.nil?
only_now = ActionDispatch::Flash.new
only_now.now[:notice] = "temporary"
raise "only flash.now retained" unless only_now.to_session_value.nil?
puts "Flash#to_session_value serializes the messages kept for the next request"
"#)
        .assert_passes();
}

#[test]
fn flash_session_value_is_known_before_lowering() {
    let errors = emit_and_run::empty_app()
        .write("app/controllers/messages_controller.rb", r#"class MessagesController < ActionController::Base
  def show
    render plain: flash.to_session_value.to_s
  end
end
"#)
        .check_errors();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn flash_session_value_is_refused_off_the_ruby_family() {
    use roundhouse::project::BuildTarget;

    for target in [BuildTarget::Rust, BuildTarget::Typescript] {
        for body in [
            "render plain: flash.to_session_value.to_s",
            "value = flash\n    render plain: value.to_session_value.to_s",
            "render plain: pending_flash.to_s",
        ] {
            let helper = if body == "render plain: pending_flash.to_s" { r#"
  private
  def pending_flash
    value = flash
    if value
      value.to_session_value
    else
      nil
    end
  end
"# } else { "" };
            let source = format!(r#"class MessagesController < ActionController::Base
  def show
    {body}
  end

  def to_session_value
    "app method"
  end
{helper}
end
"#);
            let (_, errors) = emit_and_run::empty_app()
                .write("app/controllers/messages_controller.rb", &source)
                .emit(target);
            assert!(errors.iter().any(|error| error.contains("ActionDispatch::Flash#to_session_value")), "{target:?}: {errors:?}");
        }
    }
}

#[test]
fn an_app_session_value_method_is_not_refused_as_flash() {
    use roundhouse::project::BuildTarget;

    for target in [BuildTarget::Rust, BuildTarget::Typescript] {
        let (_, errors) = emit_and_run::empty_app()
            .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"title\"\n  end\nend\n")
            .write("app/models/article.rb", "class Article < ActiveRecord::Base\n  def to_session_value\n    \"owned\"\n  end\nend\n")
            .write("app/controllers/messages_controller.rb", "class MessagesController < ActionController::Base\n  def show\n    render plain: Article.new.to_session_value\n  end\nend\n")
            .emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
    }
}
