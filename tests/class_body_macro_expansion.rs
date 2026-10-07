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

#[test]
fn unsupported_filter_macro_keeps_its_inventory_identity_and_whole_body() {
    use roundhouse::ingest::survey;

    let tree: std::collections::HashMap<_, _> = [
        (
            "app/controllers/concerns/authentication.rb",
            r#"
module Authentication
  extend ActiveSupport::Concern
  class_methods do
    def require_unauthenticated_access(**options)
      allow_unauthenticated_access **options
      before_action :redirect_signed_in_user_to_root, **options
    end
  end
end
"#,
        ),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ActionController::Base\n include Authentication\n require_unauthenticated_access only: :new\nend\n",
        ),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    let strict_app = ingest_app_from_tree(tree.clone())
        .expect("an unrecognized filter macro must not abort strict ingest");
    assert!(filters(&strict_app).is_empty());
    survey::activate();
    let result = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    let app = result.expect("survey retains the unsupported macro");
    assert!(gaps.iter().any(|gap| matches!(
        gap,
        roundhouse::ingest::IngestError::Unsupported { file, message }
            if file == "ThingsController"
                && message == "class-body macro not expanded: `require_unauthenticated_access` from Authentication holds a statement that is not filter DSL"
    )), "{gaps:?}");
    assert!(filters(&app).is_empty(), "must not expand only the callback");
    assert!(app.controllers[0].body.iter().any(|item| matches!(
        item,
        ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, roundhouse::expr::ExprNode::Send { method, .. }
                if method.as_str() == "require_unauthenticated_access")
    )));
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

fn assert_configuration_stays_unknown(concern: &str) {
    use roundhouse::expr::ExprNode;
    use roundhouse::ingest::survey;

    let strict = configuration_app(concern, "configure_window mode: :month")
        .expect("unrecognized DSL preserves the legacy strict-ingest behavior");
    assert_eq!(strict.controllers[0].class_methods().count(), 0);
    survey::activate();
    let result = configuration_app(concern, "configure_window mode: :month");
    let gaps = survey::drain();
    let app = result.expect("survey retains the unsupported call");
    assert!(gaps.iter().any(|gap| gap.to_string().contains("configure_window")), "{gaps:?}");
    assert!(!app.controllers[0].body.iter().any(|item| matches!(item,
        ControllerBodyItem::ClassMethod { .. })));
    let stored = app.controllers[0].body.iter().any(|item| {
        matches!(item, ControllerBodyItem::ClassIvarInit { .. })
    });
    if !stored {
        assert!(app.controllers[0].body.iter().any(|item| matches!(item,
            ControllerBodyItem::Unknown { expr, .. } if matches!(&*expr.node,
                ExprNode::Send { method, args, .. }
                    if method.as_str() == "configure_window" && args.len() == 1))));
    }
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
    assert_configuration_stays_unknown(&concern);
}

#[test]
fn configuration_does_not_drop_unrepresented_formals() {
    for (original, replacement) in [
        ("def configure_window(**opts)", "def configure_window(**opts, &)"),
        ("def configure_window(**opts)", "def configure_window(*, **opts)"),
        ("def configure_window(**opts)", "def configure_window((x, y), **opts)"),
        ("def window_options", "def window_options(&)"),
        ("def window_options", "def window_options(**nil)"),
    ] {
        let concern = WINDOW_SETTINGS.replace(original, replacement);
        assert_configuration_stays_unknown(&concern);
    }
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
    let app = configuration_app(&concern, "configure_window mode: :month")
        .expect("class << self is a class-method carrier");
    assert!(
        app.controllers[0].body.iter().any(|item| matches!(item, ControllerBodyItem::ClassIvarInit { .. })),
        "the includer call is stored"
    );
    for declaration in ["class_methods do", "module ClassMethods"] {
        let concern = WINDOW_SETTINGS
            .replace("class_methods do", declaration)
            .replace("def configure_window", "def self.configure_window")
            .replace("def window_options", "def self.window_options");
        let (carriers, _) = roundhouse::ingest::library_class::ingest_concern_class_method_spans(
            concern.as_bytes(), "app/controllers/concerns/window_settings.rb",
        );
        assert!(carriers.iter().all(|carrier| carrier.methods.is_empty()));
        let error = configuration_app(&concern, "configure_window mode: :month")
            .expect_err("visibility ingestion refuses the unsupported nested singleton level");
        assert!(error.to_string().contains("nested singleton"), "{error}");
    }
}

#[test]
fn configuration_preserves_visibility_wrapped_definitions() {
    use roundhouse::dialect::MethodVisibility;
    let concern = WINDOW_SETTINGS
        .replace("def configure_window", "private def configure_window")
        .replace("def window_options", "public def window_options");
    let mut app = configuration_app(&concern, "configure_window mode: :month").unwrap();
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error), "{diags:?}");
    let methods: Vec<_> = app.controllers[0].class_methods().collect();
    assert_eq!(methods.len(), 2);
    assert_eq!(methods.iter().find(|m| m.name.as_str() == "configure_window").unwrap().visibility, MethodVisibility::Private);
    assert_eq!(methods.iter().find(|m| m.name.as_str() == "window_options").unwrap().visibility, MethodVisibility::Public);
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
    assert_configuration_stays_unknown(&replaced);
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
        let stored = app.controllers[0].body.iter().any(|item| {
            matches!(item, ControllerBodyItem::ClassIvarInit { .. })
        });
        let retained = app.controllers[0].body.iter().any(|item| {
            matches!(item, ControllerBodyItem::Unknown { expr, .. } if matches!(&*expr.node, roundhouse::expr::ExprNode::Send { method, .. } if method.as_str() == "configure_window"))
        });
        assert!(stored || retained, "a refused readable store is consumed; an unreadable call stays");
    }
}

#[test]
fn store_writer_spellings_are_consumed_or_named() {
    use roundhouse::ingest::survey;
    let shapes = [
        ("bare keywords", "configure_window mode: :open, valid_steps: %w[one], default_step: \"one\", max_ahead: 0, clamp_start: true"),
        ("parentheses", "configure_window(mode: :open, valid_steps: %w[one])"),
        ("lambda arg", "configure_window default_date: ->(today) { today - 1 }, mode: :closed"),
        ("percent i", "configure_window valid_steps: %i[one two]"),
        ("string array", "configure_window valid_steps: [\"one\", \"two\"]"),
        ("symbol array", "configure_window only: [:show], except: [:index]"),
        ("true false nil", "configure_window clamp_start: true, max_ahead: 0, empty: nil"),
        ("multiline", "configure_window mode: :open,\n    valid_steps: %w[one]"),
        ("included block", "included do\n  helper :current\nend\n  configure_window mode: :open"),
        ("block stays", "configure_window(mode: :open) { :ready }"),
    ];
    for (label, call) in shapes {
        let concern = r#"
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
  def current
    @current
  end
  helper :current
end
"#;
        let controller = format!(
            "class ReportsController < BaseController\n  include WindowSettings\n  {call}\nend\n"
        );
        let tree = [
            ("app/controllers/concerns/window_settings.rb", concern),
            ("app/controllers/base_controller.rb", "class BaseController < ActionController::Base\nend\n"),
            ("app/controllers/dashboards_controller.rb", controller.as_str()),
        ]
        .into_iter()
        .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
        .collect();
        survey::activate();
        let app = ingest_app_from_tree(tree).unwrap_or_else(|err| panic!("{label}: {err}"));
        let gaps = survey::drain();
        let stored = app.controllers.iter().any(|controller| {
            controller.body.iter().any(|item| matches!(item, ControllerBodyItem::ClassIvarInit { .. }))
        });
        let unrecognized = gaps.iter().any(|gap| gap.to_string().contains("not recognized"));
        assert!(
            stored || !unrecognized,
            "{label} stayed unrecognized; gaps={gaps:?}"
        );
    }
}

#[test]
fn a_namespaced_controller_include_is_the_module_the_store_searches() {
    use roundhouse::ingest::survey;
    let concern = "module WindowSettings\n  extend ActiveSupport::Concern\n  class_methods do\n    def configure_window(**opts)\n      @window_options = opts\n    end\n    def window_options\n      @window_options || {}\n    end\n  end\n  def current\n    @current\n  end\nend\n";
    let controller = "module MarketData\n  class AnnualValuesController < BaseController\n    include WindowSettings\n    configure_window mode: :open\n  end\nend\n";
    let tree = [
        ("app/controllers/concerns/window_settings.rb", concern),
        ("app/controllers/base_controller.rb", "class BaseController < ActionController::Base\nend\n"),
        ("app/controllers/market_data/annual_values_controller.rb", controller),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("namespaced controller");
    let gaps = survey::drain();
    let controller = app.controllers.iter().find(|c| c.name.0.as_str().contains("Annual")).expect("controller");
    let includes: Vec<_> = controller.body.iter().filter_map(|item| match item {
        ControllerBodyItem::Unknown { expr, .. } => Some(format!("{:?}", expr.node).chars().take(180).collect::<String>()),
        _ => None,
    }).collect();
    let methods: Vec<_> = app.library_classes.iter().map(|lc| {
        format!("{} {:?}", lc.name.0.as_str(), lc.methods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>())
    }).collect();
    let stored = controller.body.iter().any(|item| matches!(item, ControllerBodyItem::ClassIvarInit { .. }));
    assert!(stored, "not stored\nincludes={includes:?}\nlibrary={methods:?}\ngaps={gaps:?}\nname={}", controller.name.0.as_str());
}

#[test]
fn a_concern_with_an_instance_method_still_stores_its_writer() {
    use roundhouse::ingest::survey;
    let concern = r#"
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
  def current
    @current
  end
  helper :current
end
"#;
    let controller = r#"
class ReportsController < BaseController
  include WindowSettings
  configure_window mode: :open, valid_steps: %w[one], default_step: "one"
end
"#;
    let tree = [
        ("app/controllers/concerns/window_settings.rb", concern),
        ("app/controllers/base_controller.rb", "class BaseController < ActionController::Base\nend\n"),
        ("app/controllers/dashboards_controller.rb", controller),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("concern with instance method");
    let gaps = survey::drain();
    let stored = app.controllers.iter().any(|controller| {
        controller.name.0.as_str() == "ReportsController"
            && controller.body.iter().any(|item| matches!(item, ControllerBodyItem::ClassIvarInit { .. }))
    });
    let verified = format!("{:?}", app.library_classes.iter().map(|lc| lc.name.0.as_str()).collect::<Vec<_>>());
    assert!(
        stored,
        "writer was not stored; gaps={gaps:?} library={verified}"
    );
    assert!(
        !gaps.iter().any(|gap| gap.to_string().contains("not recognized")),
        "{gaps:?}"
    );
}

#[test]
fn a_store_only_class_method_is_not_an_unrecognized_macro() {
    use roundhouse::ingest::survey;
    let concern = r#"
module WindowSettings
  extend ActiveSupport::Concern
  class_methods do
    def configure_window(**opts)
      @window_options = opts
    end
  end
end
"#;
    let controller = r#"
class ReportsController < ActionController::Base
  include WindowSettings
  configure_window(mode: :open, default_date: ->(today) { today - 1 })
  def show
  end
end
"#;
    let tree = [
        ("app/controllers/concerns/window_settings.rb", concern),
        ("app/controllers/reports_controller.rb", controller),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("store-only call continues");
    let gaps = survey::drain();
    assert!(
        !gaps.iter().any(|gap| gap.to_string().contains("not recognized")),
        "{gaps:?}"
    );
    assert!(
        app.controllers.iter().any(|controller| controller.body.iter().any(|item| {
            matches!(item, ControllerBodyItem::ClassIvarInit { .. })
        })),
        "the stored call must become class state"
    );
    let paired = r#"
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
    let paired_controller = r#"
class ReportsController < ActionController::Base
  include WindowSettings
  configure_window(mode: :closed, default_date: ->(today) { today - 1 }, valid_steps: %w[one])
  def show
  end
end
"#;
    let paired_tree = [
        ("app/controllers/concerns/window_settings.rb", paired),
        ("app/controllers/reports_controller.rb", paired_controller),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    survey::activate();
    let paired_app = ingest_app_from_tree(paired_tree).expect("paired reader continues");
    let paired_gaps = survey::drain();
    assert!(
        !paired_gaps.iter().any(|gap| gap.to_string().contains("not recognized")),
        "{paired_gaps:?}"
    );
    assert!(
        paired_app.controllers.iter().any(|controller| controller.body.iter().any(|item| {
            matches!(item, ControllerBodyItem::ClassIvarInit { .. })
        })),
        "a writer with a reader still stores the call"
    );
    let inherited = r#"
class ReportsController < BaseController
  include WindowSettings
  configure_window mode: :open, valid_steps: %w[one], default_step: "one", max_ahead: 0, clamp_start: true
  def show
  end
end
"#;
    let inherited_tree = [
        ("app/controllers/concerns/window_settings.rb", paired),
        ("app/controllers/base_controller.rb", "class BaseController < ActionController::Base\nend\n"),
        ("app/controllers/dashboards_controller.rb", inherited),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    survey::activate();
    let inherited_app = ingest_app_from_tree(inherited_tree).expect("unparenthesized inherited call");
    let inherited_gaps = survey::drain();
    assert!(
        !inherited_gaps.iter().any(|gap| gap.to_string().contains("not recognized") && gap.to_string().contains("configure_window")),
        "{inherited_gaps:?}"
    );
    assert!(
        inherited_app.controllers.iter().any(|controller| controller.name.0.as_str() == "ReportsController" && controller.body.iter().any(|item| {
            matches!(item, ControllerBodyItem::ClassIvarInit { .. })
        })),
        "unparenthesized call on a subclass must store: {:?}",
        inherited_app.controllers.iter().find(|c| c.name.0.as_str() == "ReportsController").map(|c| c.body.iter().map(|item| format!("{item:?}")).collect::<Vec<_>>())
    );
}

#[test]
fn readable_class_methods_store_keywords_blocks_and_filter_options() {
    let shapes = [
        ("configure_window mode: :month, days: 3", true),
        ("configure_window only: [:show], except: [:index], if: :ready?, unless: :draft?", true),
        ("configure_window auth: -> { current_user }", true),
        ("configure_window mode: helper", false),
        ("configure_window(mode: :month, days: 3)", true),
        ("configure_window default_date: ->(today) { today }", true),
        ("configure_window(mode: :month) { :ready }", false),
    ];
    for (call, readable) in shapes {
        let result = configuration_app(WINDOW_SETTINGS, call);
        if readable {
            let app = result.expect(call);
            assert!(
                app.controllers[0].body.iter().any(|item| matches!(item, ControllerBodyItem::ClassIvarInit { .. })),
                "{call} was not stored"
            );
        } else {
            assert!(result.is_err(), "{call} should stay ledgered whole");
        }
    }
}

#[test]
fn configuration_keeps_unrelated_class_methods_out_of_its_macro_carriers() {
    let mut app = configuration_app(WINDOW_SETTINGS, "def self.unrelated; eval('1'); end").expect("ordinary class methods are retained");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let diagnostics = roundhouse::analyze::diagnose(&app);
    assert!(diagnostics.iter().any(|d| d.message.contains("eval")), "{diagnostics:?}");
    assert!(app.controllers.iter().flat_map(|c| &c.body).any(|item| matches!(item,
        ControllerBodyItem::ClassMethod { method, configuration_slot: None, .. } if method.name.as_str() == "unrelated")));
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
fn configuration_refuses_extension_only_reopenings() {
    for separate_file in [false, true] {
        for extension_first in [false, true] {
            let extension = "module WindowSettings\n extend OtherDSL\nend\n";
            let combined = if extension_first {
                format!("{extension}{WINDOW_SETTINGS}")
            } else {
                format!("{WINDOW_SETTINGS}{extension}")
            };
            let mut files = vec![
                ("app/controllers/concerns/window_settings.rb", if separate_file { WINDOW_SETTINGS } else { &combined }),
                ("app/controllers/window_controller.rb", "class WindowController < ActionController::Base\n include WindowSettings\n configure_window mode: :month\nend\n"),
            ];
            if separate_file {
                files.push((if extension_first {
                    "app/controllers/concerns/a_extension.rb"
                } else {
                    "app/controllers/concerns/z_extension.rb"
                }, extension));
            }
            let tree = files.into_iter()
                .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
                .collect();
            let error = ingest_app_from_tree(tree)
                .expect_err("an extension-only reopen must invalidate framework identity");
            assert!(error.to_string().contains("unmodified ActiveSupport::Concern"), "{error}");
        }
    }
}

#[test]
fn blog_target_copies_configuration_source_without_emitting_state() {
    use roundhouse::project::{BuildTarget, target_files};

    let mut app = configuration_app(WINDOW_SETTINGS, "configure_window mode: :month").unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    let root = std::env::temp_dir().join(format!("roundhouse-concern-blog-{}", std::process::id()));
    std::fs::create_dir_all(root.join("app/controllers/concerns")).unwrap();
    std::fs::write(root.join("app/controllers/concerns/window_settings.rb"), WINDOW_SETTINGS).unwrap();
    let result = target_files(&app, &root, BuildTarget::Blog);
    std::fs::remove_dir_all(&root).unwrap();
    let files = result.expect("Blog is a verbatim source target, not a transpiler");
    assert_eq!(files.iter().find(|(path, _)| path == "app/controllers/concerns/window_settings.rb")
        .map(|(_, source)| source.as_str()), Some(WINDOW_SETTINGS));
    for target in [BuildTarget::Rust, BuildTarget::Roda] {
        assert!(target_files(&app, std::path::Path::new("."), target).is_err());
    }
}

#[test]
fn configuration_refuses_lexically_shadowed_framework_constants() {
    let local = WINDOW_SETTINGS.replace(
        "extend ActiveSupport::Concern",
        "ActiveSupport = String\n extend ActiveSupport::Concern",
    );
    assert!(configuration_app(&local, "configure_window mode: :month").is_err());

    let nested = format!("module Namespace\n ActiveSupport = String\n{WINDOW_SETTINGS}\nend\n");
    let tree = [
        ("app/controllers/concerns/window_settings.rb", nested.as_str()),
        ("app/controllers/window_controller.rb", "class WindowController < ActionController::Base\n include Namespace::WindowSettings\n configure_window mode: :month\nend\n"),
    ].into_iter().map(|(path, source)| (path.into(), source.as_bytes().to_vec())).collect();
    assert!(ingest_app_from_tree(tree).is_err());

    // An unrelated namespace is not on this carrier's lexical lookup path.
    let unrelated = format!("module Unrelated\n ActiveSupport = String\nend\n{WINDOW_SETTINGS}");
    assert!(configuration_app(&unrelated, "configure_window mode: :month").is_ok());
    let unrelated = format!("module Unrelated\n if true; ActiveSupport ||= String; end\nend\n{WINDOW_SETTINGS}");
    assert_eq!(configuration_app(&unrelated, "configure_window mode: :month").unwrap()
        .controllers[0].class_methods().count(), 2);

    // A root module reopening preserves the framework identity.
    let reopened = format!("module ActiveSupport; end\n{WINDOW_SETTINGS}");
    let app = configuration_app(&reopened, "configure_window mode: :month").unwrap();
    assert_eq!(app.controllers[0].class_methods().count(), 2);

    for prefix in [
        "ActiveSupport::Unrelated = 1",
        "ActiveSupport::Unrelated ||= 1",
        "ActiveSupport::Unrelated, other = 1, 2",
        "ActiveSupport::Inflector::FOO = 1",
        "module ActiveSupport; Unrelated = 1; end",
    ] {
        let tree = [
            ("lib/framework_identity.rb", prefix),
            ("app/controllers/concerns/window_settings.rb", WINDOW_SETTINGS),
            ("app/controllers/window_controller.rb", "class WindowController < ActionController::Base\n include WindowSettings\n configure_window mode: :month\nend\n"),
        ].into_iter().map(|(path, source)| (path.into(), source.as_bytes().to_vec())).collect();
        let app = ingest_app_from_tree(tree).unwrap_or_else(|error| panic!("{prefix}: {error}"));
        assert_eq!(app.controllers[0].class_methods().count(), 2, "{prefix}");
    }

    for prefix in [
        "ActiveSupport = String",
        "ActiveSupport ||= String",
        "ActiveSupport &&= String",
        "ActiveSupport += String",
        "ActiveSupport, other = String, 1",
        "ActiveSupport::Concern ||= String",
        "ActiveSupport::Concern &&= String",
        "ActiveSupport::Concern += String",
        "ActiveSupport::Concern, other = String, 1",
        "Object.new::Concern = String",
        "Object.new::Concern, other = String, 1",
        "module Object.new::ActiveSupport; end",
        "module Object.new::Concern; end",
        "if true; ActiveSupport = String; end",
        "unless false; ActiveSupport = String; end",
        "begin; ActiveSupport = String; end",
        "class ActiveSupport; end",
        "module ActiveSupport::Concern; end",
        "module ActiveSupport; module Concern; end; end",
        "module ActiveSupport; class Concern; end; end",
        "module ActiveSupport; Concern = String; end",
        "module ActiveSupport; Concern ||= String; end",
        "module ActiveSupport; Concern &&= String; end",
        "module ActiveSupport; Concern += String; end",
        "module ActiveSupport; Concern, other = String, 1; end",
        "module ActiveSupport; if true; Concern = String; end; end",
        "module Unrelated; module ::ActiveSupport::Concern; end; end",
        "WindowSettings::ActiveSupport = String",
        "module WindowSettings; module ActiveSupport; end; end",
        "ActiveSupport::Concern = String",
    ] {
        // Keep the carrier in its own file: a leading class declaration in
        // a controller concern file selects the controller-ingest path.
        let tree = [
            ("lib/framework_identity.rb", prefix),
            ("app/controllers/concerns/window_settings.rb", WINDOW_SETTINGS),
            ("app/controllers/window_controller.rb", "class WindowController < ActionController::Base\n include WindowSettings\n configure_window mode: :month\nend\n"),
        ].into_iter().map(|(path, source)| (path.into(), source.as_bytes().to_vec())).collect();
        let result = ingest_app_from_tree(tree);
        assert!(
            matches!(result, Err(roundhouse::ingest::IngestError::Unsupported { ref message, .. })
                if message.contains("unmodified ActiveSupport::Concern")),
            "identity barrier {prefix}"
        );
    }
}

#[test]
fn configuration_refuses_model_and_controller_lexical_shadows() {
    for (path, base) in [
        ("app/models/namespace.rb", "ActiveRecord::Base"),
        (
            "app/controllers/namespace_controller.rb",
            "ActionController::Base",
        ),
    ] {
        let source =
            format!("class Namespace < {base}\n ActiveSupport = String\n{WINDOW_SETTINGS}\nend\n");
        let tree = [
            (path, source.as_str()),
            ("app/controllers/window_controller.rb", "class WindowController < ActionController::Base\n include Namespace::WindowSettings\n configure_window mode: :month\nend\n"),
        ].into_iter().map(|(path, source)| (path.into(), source.as_bytes().to_vec())).collect();
        if let Ok(mut app) = ingest_app_from_tree(tree) {
            let methods = app
                .controllers
                .iter()
                .map(|c| c.class_methods().count())
                .sum::<usize>();
            roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
            let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
                .into_iter()
                .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
                .collect();
            panic!(
                "accepted {base} lexical shadow with {methods} synthesized methods; actual diagnostics: {errors:?}"
            );
        }
    }
}

#[test]
fn configuration_refuses_included_ancestor_framework_shadows() {
    let concern = format!(
        "module Shadow\n ActiveSupport = String\nend\n{}",
        WINDOW_SETTINGS.replace(
            "extend ActiveSupport::Concern",
            "extend ActiveSupport::Concern\n include Shadow\n extend ActiveSupport::Concern",
        )
    );
    let result = configuration_app(&concern, "configure_window mode: :month");
    assert!(
        result.is_err(),
        "an included module shadows the second extension"
    );

    // The superclass of an enclosing lexical class is NOT searched from
    // its nested module, unlike the innermost module's included ancestors.
    let source = format!(
        "class Parent\n ActiveSupport = String\nend\nclass Namespace < Parent\n{WINDOW_SETTINGS}\nend\n"
    );
    let tree = [
        ("app/controllers/concerns/window_settings.rb", source.as_str()),
        ("app/controllers/window_controller.rb", "class WindowController < ActionController::Base\n include Namespace::WindowSettings\n configure_window mode: :month\nend\n"),
    ].into_iter().map(|(path, source)| (path.into(), source.as_bytes().to_vec())).collect();
    assert!(ingest_app_from_tree(tree).is_ok());
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

#[test]
fn explicit_keyword_producers_bind_values_without_nested_argument_markers() {
    for (options, expected) in [("{only: %i[new create]}", vec!["new", "create"]), ("{}", vec![])] {
        let app = app_with(&format!("class ThingsController < ApplicationController\n allow_unauthenticated_access(**{options})\n def show; end\nend"));
        let skips: Vec<_> = filters(&app).into_iter().filter(|(kind, _, _, _)| *kind == FilterKind::Skip).collect();
        assert_eq!(skips.len(), 1, "explicit ** should expand: {skips:?}");
        assert_eq!(skips[0].1, "require_authentication");
        assert_eq!(skips[0].2, expected);
        assert!(skips[0].3.is_empty());
    }
}

#[test]
fn a_parameterized_rate_limit_guard_is_not_inlined_unbound() {
    use roundhouse::dialect::ControllerBodyItem;
    let source = br#"class ProbeController < ApplicationController
  rate_limit to: 5, within: 1.minute, if: ->(controller) { controller.admin? }
  def show
  end
end
"#;
    let controller = roundhouse::ingest::ingest_controller(source, "probe_controller.rb")
        .expect("ingest")
        .expect("controller");
    assert!(
        !controller.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { .. })),
        "a parameterized guard must not become a filter whose body names an unbound parameter: {:?}",
        controller.body.iter().map(|item| match item {
            ControllerBodyItem::Filter { .. } => "filter",
            ControllerBodyItem::Unknown { .. } => "unknown",
            _ => "other",
        }).collect::<Vec<_>>()
    );
}

/// The `setup_mobile!` filters `call` expands to, as their `only` lists.
/// `has_mobile_version(*actions)` peels its options with
/// `extract_options!` and reads `options[:if]`.
fn mobile_version_filters(call: &str) -> Vec<Vec<String>> {
    let concern = r#"
module MobileableConcern
  extend ActiveSupport::Concern

  module ClassMethods
    def has_mobile_version(*actions)
      options = actions.extract_options!
      before_action(:setup_mobile!, if: options[:if], only: actions)
    end
  end

  private
    def setup_mobile!
    end
end
"#;
    let controller = format!(
        "class ThingsController < ApplicationController\n  ACTIONS = %i[index show]\n  {call}\n  def index; end\n  def show; end\nend\n"
    );
    let tree = vec![
        ("app/controllers/concerns/mobileable_concern.rb", concern.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include MobileableConcern\nend\n".to_string(),
        ),
        ("app/controllers/things_controller.rb", controller),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    filters(&app)
        .into_iter()
        .filter(|(kind, target, ..)| *kind == FilterKind::Before && target == "setup_mobile!")
        .map(|(_, _, only, _)| only)
        .collect()
}

/// The call's symbols are the filter's `only`; an absent `if:` is nil,
/// no guard.
#[test]
fn rest_actions_macro_with_extract_options_expands_to_a_scoped_filter() {
    let scoped = vec![vec!["index".to_string(), "show".to_string()]];
    assert_eq!(mobile_version_filters("has_mobile_version :index, :show"), scoped);
    // A literal array splat spreads its elements.
    assert_eq!(mobile_version_filters("has_mobile_version *%i[index show]"), scoped);
    // A repeated key reads its last value, as Ruby does.
    assert_eq!(mobile_version_filters("has_mobile_version :index, :show, if: :x, if: nil"), scoped);
}

/// What expansion cannot read stays unexpanded rather than becoming a
/// broader or unguarded filter.
#[test]
fn rest_actions_macro_refuses_what_it_cannot_read() {
    for call in [
        // Unknown actions: expanding would drop them from `only`.
        "has_mobile_version *ACTIONS",
        "has_mobile_version *[:index, ACTIONS.first]",
        "has_mobile_version :index, ACTIONS.first",
        // The last `if:` is a guard this expansion does not carry.
        "has_mobile_version :index, if: nil, if: :x",
        // A computed key might be `:if`.
        "has_mobile_version :index, \"if\".to_sym => :x",
    ] {
        assert!(mobile_version_filters(call).is_empty(), "{call} expanded");
    }
}

/// `ingest::class_attribute` is all or nothing per carrier: anything in
/// `included do` beyond `class_attribute` and filter DSL, or a class
/// method using `@name` itself, leaves the macro call unexpanded.
#[test]
fn class_attribute_carrier_refuses_what_it_cannot_carry() {
    /// `(method, slot)` of each carried class_attribute method.
    fn class_attribute_methods(included: &str, writer: &str) -> Vec<(String, String)> {
        let concern = format!(
            "module Preloads\n  extend ActiveSupport::Concern\n  included do\n{included}\n  end\n  class_methods do\n    def preload(codes)\n      {writer}\n    end\n  end\nend\n"
        );
        let tree = vec![
            ("app/controllers/concerns/preloads.rb", concern),
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\nend\n".to_string(),
            ),
            (
                "app/controllers/things_controller.rb",
                "class ThingsController < ApplicationController\n  include Preloads\n  preload %w[a]\n  def show; end\nend\n"
                    .to_string(),
            ),
        ]
        .into_iter()
        .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
        .collect();
        let app = ingest_app_from_tree(tree).expect("ingest");
        let c = app
            .controllers
            .iter()
            .find(|c| c.name.0.as_str() == "ThingsController")
            .expect("ThingsController ingested");
        c.body
            .iter()
            .filter_map(|item| match item {
                ControllerBodyItem::ClassMethod {
                    method,
                    configuration_slot: Some(configuration_slot),
                    configuration_role: Some(
                        roundhouse::dialect::ClassConfigurationRole::ClassAttribute,
                    ),
                    ..
                } => Some((
                    method.name.as_str().to_string(),
                    configuration_slot.1.as_str().to_string(),
                )),
                _ => None,
            })
            .collect()
    }
    let carried = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs.iter().map(|(m, s)| (m.to_string(), s.to_string())).collect()
    };
    let attr = "    class_attribute :defs, default: []\n    before_action :run_preloads";
    let write = "self.defs += [codes]";
    assert_eq!(
        class_attribute_methods(attr, write),
        carried(&[("defs", "defs"), ("preload", "defs")]),
        "reader and macro are carried"
    );
    assert!(
        class_attribute_methods(&format!("{attr}\n    helper_method :defs"), write).is_empty(),
        "an `included` statement that is neither class_attribute nor filter DSL"
    );
    roundhouse::ingest::survey::activate();
    let _ = class_attribute_methods(&format!("{attr}\n    helper_method :defs"), write);
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|g| matches!(
            g,
            roundhouse::ingest::IngestError::Unsupported { message, .. }
                if message.contains("included do")
        )),
        "unsupported included body must be carrier-ledgered; got {gaps:?}"
    );
    assert!(
        class_attribute_methods(attr, "@defs = [codes]").is_empty(),
        "a source `@defs` is not the attribute's storage"
    );
    // Each method is keyed to the attribute it writes.
    let two = format!("{attr}\n    class_attribute :more, default: []");
    assert_eq!(
        class_attribute_methods(&two, "self.more += [codes]"),
        carried(&[("defs", "defs"), ("more", "more"), ("preload", "more")]),
    );
    assert!(
        class_attribute_methods(&two, "self.defs += [codes]; self.more += [codes]").is_empty(),
        "one method writing two attributes has no single slot"
    );
}

/// A carrier included only through another module must stay unexpanded:
/// stripping its class methods without copying them onto the controller
/// would leave the class-body call as `Unknown` / `NoMethodError`.
#[test]
fn class_attribute_carrier_included_through_another_module_is_refused() {
    let nested = r#"
module Preloads
  extend ActiveSupport::Concern
  included do
    class_attribute :defs, default: []
  end
  class_methods do
    def preload(codes)
      self.defs += [codes]
    end
  end
end
"#;
    // A bodiless `include` wrapper is dropped by library-class ingest;
    // keep an instance method so Bundle remains and records Preloads.
    let outer = r#"
module Bundle
  extend ActiveSupport::Concern
  include Preloads
  def bundle_marker
    1
  end
end
"#;
    let tree = [
        ("app/controllers/concerns/preloads.rb", nested),
        ("app/controllers/concerns/bundle.rb", outer),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  include Bundle\n  preload %w[a]\n  def show; end\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    roundhouse::ingest::survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let gaps = roundhouse::ingest::survey::drain();
    assert!(
        gaps.iter().any(|g| matches!(
            g,
            roundhouse::ingest::IngestError::Unsupported { message, .. }
                if message.contains("included through another module")
        )),
        "nested carrier must be ledgered; got {gaps:?}"
    );
    let c = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ThingsController")
        .expect("ThingsController");
    assert!(
        !c.body.iter().any(|item| matches!(
            item,
            ControllerBodyItem::ClassMethod {
                configuration_role: Some(
                    roundhouse::dialect::ClassConfigurationRole::ClassAttribute
                ),
                ..
            }
        )),
        "nested carrier must not expand onto the controller"
    );
    let preloads = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "Preloads")
        .expect("Preloads");
    assert!(
        preloads.methods.iter().any(|m| m.name.as_str() == "preload"),
        "refused carrier must keep its class methods on the module"
    );
}
