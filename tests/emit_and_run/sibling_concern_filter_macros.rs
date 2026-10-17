//! A filter macro composed across two concerns, run as emitted Ruby.
//!
//! Authentication's `allow_unauthenticated_access` calls Authorization's
//! `allow_unauthorized_access`. Ruby resolves that call on the including
//! controller, so both concerns' callbacks come off the same actions.
//! Every callback appends its name to a trace that the response returns.
//! The expected bodies are what native Rails 8.1 answers for the same app.
//! A protected action stops at `require_authentication` unless the request
//! is signed in, so the excluded actions show that both concerns' filters
//! still run there.

use super::emit_and_run;

const AUTHENTICATION: &str = r##"module Authentication
  extend ActiveSupport::Concern

  included do
    before_action :require_authentication
  end

  class_methods do
    def require_unauthenticated_access(**options)
      allow_unauthenticated_access **options
      before_action :redirect_authenticated_user, **options
    end

    def allow_unauthenticated_access(**options)
      skip_before_action :require_authentication, **options
      before_action :resume_session, **options
      allow_unauthorized_access **options
    end
  end

  private
    def require_authentication
      @trace = "#{@trace}require_authentication,"
      render plain: "#{@trace}login" unless params[:signed_in]
    end

    def resume_session
      @trace = "#{@trace}resume_session,"
    end

    def redirect_authenticated_user
      @trace = "#{@trace}redirect_authenticated_user,"
    end
end
"##;

const AUTHORIZATION: &str = r##"module Authorization
  extend ActiveSupport::Concern

  included do
    before_action :ensure_can_access_account
  end

  class_methods do
    def allow_unauthorized_access(**options)
      skip_before_action :ensure_can_access_account, **options
    end
  end

  private
    def ensure_can_access_account
      @trace = "#{@trace}ensure_can_access_account,"
    end
end
"##;

/// `(controller, macro call)`: both macros, unscoped, `only:`, and `except:`.
const CONTROLLERS: &[(&str, &str)] = &[
    ("allow_all", "allow_unauthenticated_access"),
    ("allow_only", "allow_unauthenticated_access only: :show"),
    ("allow_except", "allow_unauthenticated_access except: :destroy"),
    ("require_all", "require_unauthenticated_access"),
    ("require_only", "require_unauthenticated_access only: :show"),
    ("require_except", "require_unauthenticated_access except: :destroy"),
];

const ANONYMOUS: &str = "resume_session,";
const REDIRECTING: &str = "resume_session,redirect_authenticated_user,";
const LOGIN: &str = "require_authentication,login";
const PROTECTED: &str = "require_authentication,ensure_can_access_account,destroy";

/// `(path, verb, signed in, body)`, as native Rails 8.1 answers each request.
fn rails_responses() -> Vec<(String, &'static str, bool, String)> {
    let mut rows = Vec::new();
    for (controller, _) in CONTROLLERS {
        let open = if controller.starts_with("require") { REDIRECTING } else { ANONYMOUS };
        let destroy_is_open = controller.ends_with("_all");
        for signed_in in [false, true] {
            rows.push((format!("/{controller}/1"), "get", signed_in, format!("{open}show")));
            let destroy = match (destroy_is_open, signed_in) {
                (true, _) => format!("{open}destroy"),
                (false, false) => LOGIN.to_string(),
                (false, true) => PROTECTED.to_string(),
            };
            rows.push((format!("/{controller}/1"), "delete", signed_in, destroy));
        }
    }
    rows
}

fn sibling_concern_app() -> emit_and_run::Overlay {
    let mut overlay = emit_and_run::empty_app()
        .write("app/controllers/concerns/authentication.rb", AUTHENTICATION)
        .write("app/controllers/concerns/authorization.rb", AUTHORIZATION)
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include Authentication\n  include Authorization\nend\n",
        );
    let mut routes = String::from("Rails.application.routes.draw do\n");
    for (controller, call) in CONTROLLERS {
        let class: String = controller
            .split('_')
            .map(|part| part[..1].to_uppercase() + &part[1..])
            .collect();
        overlay = overlay.write(
            &format!("app/controllers/{controller}_controller.rb"),
            &format!(
                "class {class}Controller < ApplicationController\n  {call}\n\n  def show\n    render plain: \"#{{@trace}}show\"\n  end\n\n  def destroy\n    render plain: \"#{{@trace}}destroy\"\n  end\nend\n"
            ),
        );
        routes.push_str(&format!("  resources :{controller}, only: [:show, :destroy]\n"));
    }
    routes.push_str("end\n");
    let mut test = String::from(
        "require \"test_helper\"\n\nclass SiblingConcernMacrosControllerTest < ActionDispatch::IntegrationTest\n",
    );
    for (index, (path, verb, signed_in, body)) in rails_responses().into_iter().enumerate() {
        let params = if signed_in { ", params: { signed_in: \"1\" }" } else { "" };
        test.push_str(&format!(
            "  test \"request {index}\" do\n    {verb} \"{path}\"{params}\n    assert_equal \"{body}\", response.body\n  end\n\n"
        ));
    }
    test.push_str("end\n");
    overlay
        .write("config/routes.rb", &routes)
        .write("test/controllers/sibling_concern_macros_controller_test.rb", &test)
}

#[test]
fn a_filter_macro_composed_across_concerns_runs_like_rails() {
    sibling_concern_app()
        .run_test("test/controllers/sibling_concern_macros_controller_test.rb")
        .assert_passes();
}

/// Each macro call has its own locals. Authentication's `allow_anonymous`
/// sets `actions = :show` and Authorization's `allow_unauthorized_access`
/// sets `actions = :destroy`; Rails keeps them apart, so an anonymous
/// `destroy` still stops at `require_authentication`.
#[test]
fn nested_macro_locals_keep_an_anonymous_destroy_blocked_like_rails() {
    let authentication = AUTHENTICATION.replace(
        r#"    def require_unauthenticated_access(**options)
      allow_unauthenticated_access **options
      before_action :redirect_authenticated_user, **options
    end

    def allow_unauthenticated_access(**options)
      skip_before_action :require_authentication, **options
      before_action :resume_session, **options
      allow_unauthorized_access **options
    end
"#,
        r#"    def allow_anonymous
      actions = :show
      allow_unauthorized_access
      skip_before_action :require_authentication, only: actions
    end
"#,
    );
    let authorization = AUTHORIZATION.replace(
        r#"    def allow_unauthorized_access(**options)
      skip_before_action :ensure_can_access_account, **options
    end
"#,
        r#"    def allow_unauthorized_access
      actions = :destroy
      skip_before_action :ensure_can_access_account, only: actions
    end
"#,
    );
    assert!(authentication.contains("allow_anonymous") && authorization.contains("actions = :destroy"));
    // As native Rails 8.1 answers each request.
    let rails_responses = [
        ("get", false, "ensure_can_access_account,show"),
        ("get", true, "ensure_can_access_account,show"),
        ("delete", false, "require_authentication,login"),
        ("delete", true, "require_authentication,destroy"),
    ];
    let mut test = String::from(
        "require \"test_helper\"\n\nclass AnonymousThingsControllerTest < ActionDispatch::IntegrationTest\n",
    );
    for (index, (verb, signed_in, body)) in rails_responses.into_iter().enumerate() {
        let params = if signed_in { ", params: { signed_in: \"1\" }" } else { "" };
        test.push_str(&format!(
            "  test \"request {index}\" do\n    {verb} \"/things/1\"{params}\n    assert_equal \"{body}\", response.body\n  end\n\n"
        ));
    }
    test.push_str("end\n");
    emit_and_run::empty_app()
        .write("app/controllers/concerns/authentication.rb", &authentication)
        .write("app/controllers/concerns/authorization.rb", &authorization)
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include Authentication\n  include Authorization\nend\n",
        )
        .write(
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  allow_anonymous\n\n  def show\n    render plain: \"#{@trace}show\"\n  end\n\n  def destroy\n    render plain: \"#{@trace}destroy\"\n  end\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :things, only: [:show, :destroy]\nend\n")
        .write("test/controllers/anonymous_things_controller_test.rb", &test)
        .run_test("test/controllers/anonymous_things_controller_test.rb")
        .assert_passes();
}
