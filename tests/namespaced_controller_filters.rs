//! A controller declared inside a module must get the SAME synthesized
//! `process_action` dispatcher as a top-level one: the inherited
//! filter chain (ApplicationController's), the intermediate namespaced
//! base's filters, its own before_actions, `only:`/`except:`/`skip_`,
//! and the `rescue_from` wrapper.
//!
//! A relative superclass (`module Ns; class XController <
//! BaseController`) was left bare, so the ancestry walk matched no
//! parent and the dispatcher's preamble came out EMPTY — no
//! ApplicationController filters, no intermediate base's filters.
//!
//! Resolving it is Ruby's lexical constant lookup, not a walk over the
//! class's name: a compact `class Admin::XController < BaseController`
//! at top level has only the top level in scope, so with both
//! `::BaseController` and `Admin::BaseController` defined it inherits
//! the top-level one. The second fixture below pins every spelling
//! against competing bases.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const APPLICATION_CONTROLLER: &str = r#"class ApplicationController < ActionController::Base
  before_action :require_authentication

  private
    def require_authentication
      @authenticated = true
    end
end
"#;

const NS_BASE: &str = r#"module Ns
  class BaseController < ApplicationController
    before_action :set_namespace
    rescue_from ActiveRecord::RecordNotFound, with: :not_found

    private
      def set_namespace
        @namespace_set = true
      end

      def not_found
        head :not_found
      end
  end
end
"#;

const NS_WIDGETS: &str = r#"module Ns
  class WidgetsController < BaseController
    before_action :set_widget, only: [:show]
    before_action :audit, except: [:index]
    skip_before_action :require_authentication, only: [:index]

    def index
    end

    def show
    end

    private
      def set_widget
        @widget = 1
      end

      def audit
        @audited = true
      end
  end
end
"#;

const THINGS: &str = r#"class ThingsController < ApplicationController
  def index
  end
end
"#;

fn emitted() -> HashMap<String, String> {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"things\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/models/thing.rb", "class Thing < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/controllers/ns/base_controller.rb", NS_BASE),
        ("app/controllers/ns/widgets_controller.rb", NS_WIDGETS),
        ("app/controllers/things_controller.rb", THINGS),
        ("config/routes.rb", "Rails.application.routes.draw do\n  namespace :ns do\n    resources :widgets, only: [:index, :show]\n  end\n  resources :things, only: [:index]\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_controllers(&app);
    let get = |name: &str| {
        files
            .iter()
            .find(|f| f.path.to_string_lossy().ends_with(name))
            .map(|f| f.content.clone())
            .unwrap_or_else(|| panic!("{name}"))
    };
    [
        ("application_controller.rb", "app/controllers/application_controller.rb"),
        ("base_controller.rb", "app/controllers/ns/base_controller.rb"),
        ("widgets_controller.rb", "app/controllers/ns/widgets_controller.rb"),
        ("things_controller.rb", "app/controllers/things_controller.rb"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), get(v)))
    .collect()
}

#[test]
fn a_namespaced_controller_gets_the_synthesized_dispatcher() {
    let files = emitted();
    let widgets = &files["widgets_controller.rb"];
    assert!(
        widgets.contains("def process_action(action_name)"),
        "a module-nested controller must get the dispatcher:\n{widgets}"
    );

    let dispatch = widgets.split("def process_action").nth(1).expect("dispatcher");
    // Rails runs the chain root-most ancestor first, then the intermediate
    // namespaced base's filters. Both live in the dispatcher preamble.
    let auth = dispatch.find("require_authentication").expect("inherited filter");
    let ns = dispatch.find("set_namespace").expect("intermediate-base filter");
    assert!(auth < ns, "ApplicationController filters run before the namespace base's:\n{widgets}");
    assert!(dispatch.contains("case action_name"), "no dispatch:\n{widgets}");
}

#[test]
fn own_filters_are_inlined_into_the_actions_they_guard() {
    let files = emitted();
    let widgets = &files["widgets_controller.rb"];
    // Own filters whose targets are this controller's own private
    // methods are inlined into the action bodies (the same treatment a
    // top-level controller gets), with their `only:`/`except:` honoured.
    let show = widgets.split("def show").nth(1).and_then(|s| s.split("def ").next()).unwrap_or("");
    let index = widgets.split("def index").nth(1).and_then(|s| s.split("def ").next()).unwrap_or("");
    assert!(show.contains("@widget = 1"), "only: [:show] must inline into show:\n{widgets}");
    assert!(show.contains("@audited = true"), "except: [:index] must inline into show:\n{widgets}");
    assert!(!index.contains("@widget = 1"), "only: [:show] must not inline into index:\n{widgets}");
    assert!(!index.contains("@audited = true"), "except: [:index] must not inline into index:\n{widgets}");
}

#[test]
fn the_rescue_from_wrapper_covers_a_namespaced_dispatcher() {
    let files = emitted();
    let widgets = &files["widgets_controller.rb"];
    let dispatch = widgets.split("def process_action").nth(1).expect("dispatcher");
    assert!(dispatch.contains("begin"), "the dispatcher is wrapped:\n{widgets}");
    assert!(
        dispatch.contains("rescue ActiveRecord::RecordNotFound"),
        "the inherited handler's exception must be rescued:\n{widgets}"
    );
    assert!(dispatch.contains("not_found"), "…running the handler:\n{widgets}");
}

#[test]
fn a_relative_superclass_name_resolves_to_the_namespaced_base() {
    // `class WidgetsController < BaseController` inside `module Ns` means
    // `Ns::BaseController`; if the parent were left as a bare
    // `BaseController`, the intermediate base's filter never reaches the
    // chain.
    let files = emitted();
    let widgets = &files["widgets_controller.rb"];
    assert!(
        widgets.contains("def process_action") && widgets.contains("set_namespace"),
        "relative superclass not resolved:\n{widgets}"
    );
}

#[test]
fn skip_before_action_is_honoured_for_a_namespaced_controller() {
    let files = emitted();
    let widgets = &files["widgets_controller.rb"];
    let dispatch = widgets.split("def process_action").nth(1).expect("dispatcher");
    // `skip_before_action :require_authentication, only: [:index]` — the
    // inherited filter survives, but is guarded off for `index`.
    assert!(
        dispatch.contains("require_authentication"),
        "the skip must narrow, not remove:\n{widgets}"
    );
    assert!(
        dispatch.contains("[:index]"),
        "the only:-guard of the skip must be emitted:\n{widgets}"
    );
}

#[test]
fn the_top_level_control_is_unchanged() {
    let files = emitted();
    let things = &files["things_controller.rb"];
    assert!(
        things.contains("def process_action(action_name)"),
        "top-level controller regressed:\n{things}"
    );
    assert!(things.contains("require_authentication"), "inherited filter missing:\n{things}");
}

// Competing bases: the same bare `BaseController` names a different
// class depending on how the subclass is written.

const TOP_BASE: &str = r#"class BaseController < ApplicationController
  before_action :top_gate

  private
    def top_gate
      @gate = :top
    end
end
"#;

const ADMIN_BASE: &str = r#"module Admin
  class BaseController < ApplicationController
    before_action :admin_gate

    private
      def admin_gate
        @gate = :admin
      end
  end
end
"#;

const ADMIN_REPORTS_BASE: &str = r#"class Admin::Reports::BaseController < ApplicationController
  before_action :reports_gate

  private
    def reports_gate
      @gate = :reports
    end
end
"#;

fn action_controller(head: &str, tail: &str) -> String {
    format!("{head}\n  def index\n  end\n{tail}\n")
}

fn competing_bases() -> (roundhouse::App, HashMap<String, String>) {
    let controllers: Vec<(&str, String)> = vec![
        ("application_controller.rb", APPLICATION_CONTROLLER.to_string()),
        ("base_controller.rb", TOP_BASE.to_string()),
        ("admin/base_controller.rb", ADMIN_BASE.to_string()),
        ("admin/reports/base_controller.rb", ADMIN_REPORTS_BASE.to_string()),
        // Compact, top level: only `::` is in lexical scope.
        (
            "admin/widgets_controller.rb",
            action_controller("class Admin::WidgetsController < BaseController", "end"),
        ),
        // Nested: `Admin` is in scope.
        (
            "admin/reports_controller.rb",
            action_controller("module Admin\nclass ReportsController < BaseController", "end\nend"),
        ),
        // Mixed: nesting is [Admin]; the `Reports::` prefix opens nothing.
        (
            "admin/reports/exports_controller.rb",
            action_controller(
                "module Admin\nclass Reports::ExportsController < BaseController",
                "end\nend",
            ),
        ),
        // Fully nested: [Admin::Reports, Admin] — innermost wins.
        (
            "admin/reports/summaries_controller.rb",
            action_controller(
                "module Admin\nmodule Reports\nclass SummariesController < BaseController",
                "end\nend\nend",
            ),
        ),
        // Same name one level down: `Admin::NotesController` is the
        // class being declared, never its own superclass.
        (
            "notes_controller.rb",
            action_controller("class NotesController < ApplicationController", "end"),
        ),
        (
            "admin/notes_controller.rb",
            action_controller("module Admin\nclass NotesController < NotesController", "end\nend"),
        ),
        // Rooted: `::BaseController` skips the nesting.
        (
            "admin/audits_controller.rb",
            action_controller(
                "module Admin\nclass AuditsController < ::BaseController",
                "end\nend",
            ),
        ),
    ];
    let routes = "Rails.application.routes.draw do\n  namespace :admin do\n    resources :widgets, only: [:index]\n    resources :reports, only: [:index]\n    resources :audits, only: [:index]\n    resources :notes, only: [:index]\n    namespace :reports do\n      resources :exports, only: [:index]\n      resources :summaries, only: [:index]\n    end\n  end\nend\n";
    let mut tree: HashMap<PathBuf, Vec<u8>> = controllers
        .into_iter()
        .map(|(p, c)| (PathBuf::from(format!("app/controllers/{p}")), c.into_bytes()))
        .collect();
    tree.insert(PathBuf::from("config/routes.rb"), routes.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let parents = app
        .controllers
        .iter()
        .map(|c| {
            let parent = c.parent.as_ref().map(|p| p.0.as_str().to_string());
            (c.name.0.as_str().to_string(), parent.unwrap_or_default())
        })
        .collect();
    roundhouse::session::analyze_and_lower(&mut app);
    (app, parents)
}

#[test]
fn a_relative_superclass_resolves_against_the_lexical_nesting_not_the_name() {
    let (_, parents) = competing_bases();
    for (controller, parent) in [
        ("Admin::WidgetsController", "BaseController"),
        ("Admin::ReportsController", "Admin::BaseController"),
        ("Admin::Reports::ExportsController", "Admin::BaseController"),
        ("Admin::Reports::SummariesController", "Admin::Reports::BaseController"),
        ("Admin::AuditsController", "BaseController"),
        ("Admin::NotesController", "NotesController"),
    ] {
        assert_eq!(parents[controller], parent, "{controller}'s superclass");
    }
}

#[test]
fn each_spelling_inherits_its_own_base_filters() {
    let (app, _) = competing_bases();
    let files = ruby::emit_lowered_controllers(&app);
    for (file, gate) in [
        ("admin/widgets_controller.rb", "top_gate"),
        ("admin/reports_controller.rb", "admin_gate"),
        ("admin/reports/exports_controller.rb", "admin_gate"),
        ("admin/reports/summaries_controller.rb", "reports_gate"),
        ("admin/audits_controller.rb", "top_gate"),
    ] {
        let content = &files
            .iter()
            .find(|f| f.path.to_string_lossy().ends_with(&format!("app/controllers/{file}")))
            .unwrap_or_else(|| panic!("{file}"))
            .content;
        let dispatch = content
            .split("def process_action")
            .nth(1)
            .unwrap_or_else(|| panic!("{file}: no dispatcher\n{content}"));
        for other in ["top_gate", "admin_gate", "reports_gate"] {
            assert_eq!(
                dispatch.contains(other),
                other == gate,
                "{file} must run {gate} and no other base's gate:\n{content}"
            );
        }
    }
}
