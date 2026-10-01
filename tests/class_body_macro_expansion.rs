//! A concern's class-side filter macro, run at compile time
//! (`ingest::app::expand_class_body_macros`).
//!
//! `allow_unauthenticated_access` is a method the Authentication concern
//! exports through `class_methods do`, whose body is filter DSL. Ingest
//! binds the call's arguments to the macro's parameters, substitutes,
//! and recognizes the result as Filters.
//!
//! The BARE call — no arguments at all — is the case these tests exist
//! for. Its `**options` parameter has nothing to bind to, and an
//! unbound parameter left the substituted body holding a free variable
//! that `filters_from_macro_body` could not recognize, so the whole
//! macro was dropped. Dropping is the safe direction by design
//! (all-or-nothing: half-expanding an auth macro fails OPEN), but the
//! cost was real — campfire's FirstRunsController kept
//! `require_authentication`, so `/first_run` redirected to
//! `/session/new`, which redirects back to `/first_run`.
//!
//! An unsupplied `**options` means `{}` in Ruby, which makes the skip
//! UNSCOPED — off every action, not off none.

use roundhouse::App;
use roundhouse::dialect::{ControllerBodyItem, FilterKind};
use roundhouse::ingest::ingest_app_from_tree;

const AUTHENTICATION: &str = r#"
module Authentication
  extend ActiveSupport::Concern

  included do
    before_action :require_authentication
  end

  class_methods do
    def allow_unauthenticated_access(**options)
      skip_before_action :require_authentication, **options
    end
  end

  private
    def require_authentication
      redirect_to "/session/new"
    end
end
"#;

const APPLICATION_CONTROLLER: &str = r#"
class ApplicationController < ActionController::Base
  include Authentication
end
"#;

fn app_with(controller_src: &str) -> App {
    let tree = vec![
        (
            std::path::PathBuf::from("app/controllers/concerns/authentication.rb"),
            AUTHENTICATION.as_bytes().to_vec(),
        ),
        (
            std::path::PathBuf::from("app/controllers/application_controller.rb"),
            APPLICATION_CONTROLLER.as_bytes().to_vec(),
        ),
        (
            std::path::PathBuf::from("app/controllers/things_controller.rb"),
            controller_src.as_bytes().to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

/// Every filter on ThingsController, as `(kind, target, only, except)`.
fn filters(app: &App) -> Vec<(FilterKind, String, Vec<String>, Vec<String>)> {
    let c = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ThingsController")
        .expect("ThingsController ingested");
    c.body
        .iter()
        .filter_map(|item| match item {
            ControllerBodyItem::Filter { filter, .. } => Some((
                filter.kind.clone(),
                filter.target.as_str().to_string(),
                filter.only.iter().map(|s| s.as_str().to_string()).collect(),
                filter
                    .except
                    .iter()
                    .map(|s| s.as_str().to_string())
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn bare_macro_call_expands_to_an_unscoped_skip() {
    let app = app_with(
        r#"
class ThingsController < ApplicationController
  allow_unauthenticated_access

  def show
  end
end
"#,
    );
    let skips: Vec<_> = filters(&app)
        .into_iter()
        .filter(|(kind, _, _, _)| *kind == FilterKind::Skip)
        .collect();
    assert_eq!(
        skips.len(),
        1,
        "bare macro should expand to one skip: {skips:?}"
    );
    let (_, target, only, except) = &skips[0];
    assert_eq!(target, "require_authentication");
    assert!(
        only.is_empty() && except.is_empty(),
        "an unsupplied **options means `{{}}` — the skip is UNSCOPED, \
         so it comes off every action: {only:?} / {except:?}"
    );
}

#[test]
fn scoped_macro_call_still_narrows_the_skip() {
    // The shape that already worked, kept honest: a supplied `only:` must
    // NOT be widened to an unscoped skip by the new binding.
    let app = app_with(
        r#"
class ThingsController < ApplicationController
  allow_unauthenticated_access only: %i[new create]

  def new
  end

  def show
  end
end
"#,
    );
    let skips: Vec<_> = filters(&app)
        .into_iter()
        .filter(|(kind, _, _, _)| *kind == FilterKind::Skip)
        .collect();
    assert_eq!(skips.len(), 1, "expected one skip: {skips:?}");
    let (_, target, only, _) = &skips[0];
    assert_eq!(target, "require_authentication");
    assert_eq!(
        only,
        &vec!["new".to_string(), "create".to_string()],
        "a scoped skip must stay scoped — widening it would sign users \
         out of authentication on every action"
    );
}

#[test]
fn a_controller_without_the_macro_keeps_the_filter() {
    let app = app_with(
        r#"
class ThingsController < ApplicationController
  def show
  end
end
"#,
    );
    let skips: Vec<_> = filters(&app)
        .into_iter()
        .filter(|(kind, _, _, _)| *kind == FilterKind::Skip)
        .collect();
    assert!(skips.is_empty(), "no macro call means no skip: {skips:?}");
}

const WINDOW_SETTINGS: &str = r#"
module WindowSettings
  extend ActiveSupport::Concern
  class_methods do
    def configure_window(**opts)
      @window_options = opts
    end
    def window_options
      @window_options || {}
    end
  end
end
"#;

#[test]
fn a_reopened_filter_macro_uses_its_latest_definition() {
    let concern = format!(
        "{AUTHENTICATION}\nmodule Authentication\n class_methods do\n def allow_unauthenticated_access(**options)\n skip_before_action :replacement_authentication, **options\n end\n end\nend\n"
    );
    let tree = [
        (
            "app/controllers/concerns/authentication.rb",
            concern.as_str(),
        ),
        (
            "app/controllers/application_controller.rb",
            APPLICATION_CONTROLLER,
        ),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n allow_unauthenticated_access\nend\n",
        ),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    let app = ingest_app_from_tree(tree).unwrap();
    let skipped: Vec<_> = filters(&app)
        .into_iter()
        .filter(|(kind, _, _, _)| *kind == FilterKind::Skip)
        .map(|(_, target, _, _)| target)
        .collect();
    assert_eq!(skipped, ["replacement_authentication"]);
}

fn configuration_app(concern: &str, call: &str) -> Result<App, roundhouse::ingest::IngestError> {
    let controller = format!(
        r#"
class WindowController < ActionController::Base
  include WindowSettings
  {call}
  def show
    render json: self.class.window_options
  end
end
"#
    );
    let tree = [
        ("app/controllers/concerns/window_settings.rb", concern),
        ("app/controllers/window_controller.rb", &controller),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    ingest_app_from_tree(tree)
}

#[test]
fn finite_configuration_is_class_state_not_an_action_or_instance_field() {
    use roundhouse::expr::{ExprNode, LValue};
    let mut app =
        configuration_app(WINDOW_SETTINGS, "configure_window mode: :month, days: 3").unwrap();
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diags
            .iter()
            .all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "{diags:?}"
    );
    let controller = &app.controllers[0];
    assert_eq!(
        controller
            .actions()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["show"]
    );
    assert_eq!(controller.class_methods().count(), 2);
    let init = controller
        .body
        .iter()
        .find_map(|item| match item {
            ControllerBodyItem::ClassIvarInit { expr, .. } => Some(expr),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(&*init.node, ExprNode::Assign { target: LValue::Ivar { name }, .. } if name.as_str() == "window_options")
    );
    let reader = controller
        .class_methods()
        .find(|m| m.name.as_str() == "window_options")
        .unwrap();
    assert!(
        matches!(&reader.signature, Some(roundhouse::ty::Ty::Fn { ret, .. }) if matches!(&**ret, roundhouse::ty::Ty::Hash { .. }))
    );
    for target in [
        roundhouse::project::BuildTarget::Rust,
        roundhouse::project::BuildTarget::Roda,
    ] {
        assert!(
            roundhouse::project::target_files(&app, std::path::Path::new("."), target).is_err()
        );
    }
}

#[test]
fn configuration_does_not_admit_dynamic_or_positional_arguments() {
    for call in [
        "configure_window({mode: :month})",
        "configure_window mode: ENV[\"MODE\"]",
        "configure_window **options",
    ] {
        assert!(
            configuration_app(WINDOW_SETTINGS, call).is_err(),
            "incorrectly accepted {call}"
        );
    }
}

#[test]
fn configuration_does_not_drop_extra_macro_effects() {
    let concern = WINDOW_SETTINGS.replace(
        "@window_options = opts",
        "@window_options = opts\n      puts :effect",
    );
    assert!(configuration_app(&concern, "configure_window mode: :month").is_err());
}

#[test]
fn configuration_does_not_drop_inclusion_time_storage_effects() {
    for effect in [
        "@window_options = {mode: :hidden}",
        "configure_window mode: :hidden",
    ] {
        let concern = WINDOW_SETTINGS.replace(
            "class_methods do",
            &format!("included do\n {effect}\nend\n class_methods do"),
        );
        let error = configuration_app(&concern, "")
            .expect_err("included callback would change the default state");
        assert!(error.to_string().contains("class configuration"), "{error}");
    }
    let concern = WINDOW_SETTINGS.replace(
        "class_methods do",
        "included do\n before_action :marker\nend\n def marker; :observed; end\n class_methods do",
    );
    let app = configuration_app(&concern, "configure_window mode: :month")
        .expect("the existing complete filter DSL still coexists");
    assert_eq!(app.controllers[0].class_methods().count(), 2);
    assert!(
        app.controllers[0]
            .filters()
            .any(|f| f.target.as_str() == "marker")
    );
}

#[test]
fn configuration_does_not_admit_class_body_reads_or_method_overrides() {
    for call in [
        "SNAPSHOT = window_options\nconfigure_window mode: :month",
        "configure_window mode: :month\nSNAPSHOT = window_options",
        "configure_window mode: :month\nSNAPSHOT = @window_options",
        "@window_options = {}\nconfigure_window mode: :month",
        "configure_window { puts :ignored }",
        "def self.window_options; {mode: :custom}; end\nconfigure_window mode: :month",
    ] {
        assert!(
            configuration_app(WINDOW_SETTINGS, call).is_err(),
            "incorrectly accepted {call}"
        );
    }
}

#[test]
fn a_module_singleton_is_not_a_concern_carrier() {
    let concern = WINDOW_SETTINGS.replace("class_methods do", "class << self");
    assert!(configuration_app(&concern, "configure_window mode: :month").is_err());
    for declaration in ["class_methods do", "module ClassMethods"] {
        let concern = WINDOW_SETTINGS
            .replace("class_methods do", declaration)
            .replace("def configure_window", "def self.configure_window")
            .replace("def window_options", "def self.window_options");
        let error = configuration_app(&concern, "configure_window mode: :month")
            .expect_err("carrier singletons do not extend includers");
        assert!(error.to_string().contains("not expanded"), "{error}");
    }
}

#[test]
fn configuration_obeys_carrier_spans_and_reopening_precedence() {
    // A same-named singleton is separate from the ClassMethods carrier;
    // an actual carrier reopening, in contrast, replaces its own method.
    let singleton = format!(
        "{WINDOW_SETTINGS}\nmodule WindowSettings\n def self.window_options; {{mode: :wrong}}; end\nend\n"
    );
    let nested = WINDOW_SETTINGS.replace("class_methods do", "module ClassMethods");
    for concern in [&singleton, &nested] {
        let app = configuration_app(concern, "configure_window mode: :month").unwrap();
        assert_eq!(app.controllers[0].class_methods().count(), 2);
    }
    let replaced = format!(
        "{WINDOW_SETTINGS}\nmodule WindowSettings\n class_methods do\n def window_options; {{mode: :wrong}}; end\n end\nend\n"
    );
    assert!(configuration_app(&replaced, "configure_window mode: :month").is_err());
}

#[test]
fn configuration_refusals_preserve_the_survey_and_original_body() {
    use roundhouse::ingest::survey;
    for call in [
        "configure_window mode: :month\nconfigure_window mode: ENV[\"MODE\"]",
        "SNAPSHOT = window_options\nconfigure_window mode: :month",
    ] {
        survey::activate();
        let result = configuration_app(WINDOW_SETTINGS, call);
        let gaps = survey::drain();
        let app = result.expect("survey must retain the app");
        assert!(
            gaps.iter()
                .any(|gap| gap.to_string().contains("class configuration")),
            "{gaps:?}"
        );
        assert_eq!(app.controllers[0].class_methods().count(), 0);
        assert!(
            !app.controllers[0]
                .body
                .iter()
                .any(|item| matches!(item, ControllerBodyItem::ClassIvarInit { .. }))
        );
    }
}

#[test]
fn finite_configuration_does_not_admit_unrelated_controller_singletons() {
    assert!(configuration_app(WINDOW_SETTINGS, "def self.unrelated; eval('1'); end").is_err());
}

#[test]
fn configuration_refuses_forward_includes_and_cross_carrier_storage_aliases() {
    let alias = WINDOW_SETTINGS
        .replace("WindowSettings", "OtherSettings")
        .replace("configure_window", "configure_other")
        .replace("def window_options", "def other_options");
    for (body, expected) in [
        (
            "configure_window mode: :month\ninclude WindowSettings",
            "precedes",
        ),
        (
            "include WindowSettings\ninclude OtherSettings\nconfigure_window mode: :month\nconfigure_other days: 0",
            "aliased",
        ),
    ] {
        let controller = format!("class WindowController < ActionController::Base\n{body}\nend\n");
        let tree = [
            (
                "app/controllers/concerns/window_settings.rb",
                WINDOW_SETTINGS,
            ),
            ("app/controllers/concerns/other_settings.rb", alias.as_str()),
            ("app/controllers/window_controller.rb", controller.as_str()),
        ]
        .into_iter()
        .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
        .collect();
        let error = ingest_app_from_tree(tree).expect_err("unsupported receiver contract");
        assert!(
            error.to_string().contains(expected),
            "wrong refusal for {body}: {error}"
        );
    }
}

#[test]
fn configuration_refuses_extra_carrier_state_access_and_instance_name_collisions() {
    for method in [
        "def reset_window; @window_options = {}; end",
        "def options_alias; @window_options; end",
        "def replace_window; configure_window mode: :hidden; end",
    ] {
        let concern =
            WINDOW_SETTINGS.replace("class_methods do", &format!("class_methods do\n{method}"));
        let error = configuration_app(&concern, "configure_window mode: :month").unwrap_err();
        assert!(
            error.to_string().contains("additional carrier access"),
            "{error}"
        );
    }
    for method in [
        "def configure_window(value); value.upcase; end",
        "def window_options; :instance; end",
    ] {
        let error = configuration_app(
            WINDOW_SETTINGS,
            &format!("{method}\nconfigure_window mode: :month"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("name collision"), "{error}");
    }
}

#[test]
fn configuration_refuses_an_inherited_instance_method_name_collision() {
    let tree = [
        ("app/controllers/concerns/window_settings.rb", WINDOW_SETTINGS),
        ("app/controllers/parent_controller.rb", "class ParentController < ActionController::Base\n def configure_window(value); value.upcase; end\nend\n"),
        ("app/controllers/child_controller.rb", "class ChildController < ParentController\n include WindowSettings\n configure_window mode: :month\nend\n"),
    ].into_iter().map(|(path, source)| (path.into(), source.as_bytes().to_vec())).collect();
    let error = ingest_app_from_tree(tree).expect_err("receiver-kind collision through parent");
    assert!(error.to_string().contains("name collision"), "{error}");
}

#[test]
fn configuration_requires_the_actual_unmodified_concern_api() {
    let plain = WINDOW_SETTINGS.replace("  extend ActiveSupport::Concern\n", "");
    let late = format!(
        "{}\n extend ActiveSupport::Concern\nend\n",
        plain.trim_end().strip_suffix("end").unwrap()
    );
    for concern in [
        plain.clone(),
        plain.replace("class_methods do", "module ClassMethods"),
        late,
        plain.replace(
            "class_methods do",
            "def self.class_methods; end\n class_methods do",
        ),
        WINDOW_SETTINGS.replace(
            "class_methods do",
            "def self.class_methods; end\n class_methods do",
        ),
        WINDOW_SETTINGS.replace(
            "class_methods do",
            "def self.append_features(base); end\n class_methods do",
        ),
        WINDOW_SETTINGS.replace("class_methods do", "extend OtherDSL\n class_methods do"),
    ] {
        let error = configuration_app(&concern, "configure_window mode: :month")
            .expect_err("must not invent Concern semantics");
        assert!(
            error
                .to_string()
                .contains("unmodified ActiveSupport::Concern"),
            "wrong refusal: {error}"
        );
    }
}

#[test]
fn configuration_requires_concern_identity_through_dependency_wrappers() {
    for (wrapper, supported) in [
        (
            "module WrappedSettings\n extend ActiveSupport::Concern\n include WindowSettings\n def marker; :wrapper; end\nend\n",
            true,
        ),
        (
            "module WrappedSettings\n include WindowSettings\n def marker; :wrapper; end\nend\n",
            false,
        ),
        (
            "module WrappedSettings\n include WindowSettings\n extend ActiveSupport::Concern\n def marker; :wrapper; end\nend\n",
            false,
        ),
    ] {
        let tree = [
            ("app/controllers/concerns/window_settings.rb", WINDOW_SETTINGS),
            ("app/controllers/concerns/wrapped_settings.rb", wrapper),
            ("app/controllers/window_controller.rb", "class WindowController < ActionController::Base\n include WrappedSettings\n configure_window mode: :month\nend\n"),
        ].into_iter().map(|(path, source)| (path.into(), source.as_bytes().to_vec())).collect();
        let result = ingest_app_from_tree(tree);
        if supported {
            let app = result.unwrap();
            assert_eq!(
                app.controllers[0].class_methods().count(),
                2,
                "{:?}",
                app.library_classes
            );
        } else {
            let error =
                result.expect_err("a plain wrapper cannot carry class methods to an includer");
            assert!(
                error
                    .to_string()
                    .contains("unmodified ActiveSupport::Concern"),
                "{error}"
            );
        }
    }
}
