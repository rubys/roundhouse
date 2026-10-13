//! A block-form `around_action` — `around_action(only: :create) do
//! |_controller, block| block.call ensure response.headers['X-Quota'] =
//! '9' end` — used to vanish with no survey gap at all: nothing ran in
//! `process_action`, and the response never carried the header
//! (rubys/roundhouse#778).
//!
//! `parse_filter_call` only ever typed a Symbol target, so the block
//! form fell through to `Unknown`; `report_unrecognized_controller_macros`
//! only ever looked at BLOCKLESS calls, so a block-attached one never
//! reached it either. Two gaps in the same shape, not one.
//!
//! Rails 8.1 calls an arity-2 around block as `instance_exec(controller,
//! continuation)`: the first param rebinds `self`; the second is a
//! callable whose bare `.call` resumes the chain — exactly what a NAMED
//! around filter's own method body already does with a bare `yield`
//! (see `tests/around_after_filters.rs`). `ingest::controller::
//! around_block_filter` synthesizes exactly that kind of private method
//! and registers it as an ordinary named `Around` filter, so it rides the
//! SAME dispatch a hand-written `around_action :method` already does.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{ControllerBodyItem, FilterKind};
use roundhouse::emit::ruby;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};
use roundhouse::App;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app_tree(widgets_body: &str) -> HashMap<PathBuf, Vec<u8>> {
    let files: Vec<(&str, String)> = vec![
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/controllers/widgets_controller.rb", widgets_body.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :show, :create]\nend\n"
                .to_string(),
        ),
    ];
    files.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect()
}

fn build(widgets_body: &str) -> (App, Vec<IngestError>) {
    survey::activate();
    let result = ingest_app_from_tree(app_tree(widgets_body));
    let gaps = survey::drain();
    (result.expect("ingest must not hard-fail"), gaps)
}

fn widgets_controller(app: &App) -> &roundhouse::dialect::Controller {
    app.controllers
        .iter()
        .find(|c| c.name.0.as_str() == "WidgetsController")
        .expect("WidgetsController ingested")
}

fn emitted_src(mut app: App) -> String {
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_controllers(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("widgets_controller.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("widgets_controller.rb not emitted"))
}

fn has_gap(gaps: &[IngestError], needle: &str) -> bool {
    gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains(needle)))
}

// ---------------------------------------------------------------------
// Accepted: Commit 1 — lowering
// ---------------------------------------------------------------------

const BASIC: &str = r#"class WidgetsController < ApplicationController
  around_action(only: :create) do |_controller, block|
    block.call
  ensure
    response.headers['X-Quota'] = '9'
  end

  def index
  end

  def create
  end
end
"#;

#[test]
fn a_block_form_around_action_lowers_to_a_synthesized_method_that_yields() {
    let (app, gaps) = build(BASIC);
    assert!(!has_gap(&gaps, "around_action"), "{gaps:?}");
    let c = widgets_controller(&app);
    // No Unknown item survives: the call fully lowered.
    assert!(
        !c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "around_action"))),
        "a lowered around_action must not stay Unknown: {:?}",
        c.body
    );
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { filter, .. }
            if filter.kind == FilterKind::Around)),
        "a named Around filter must be registered: {:?}",
        c.body
    );
    let src = emitted_src(app);
    assert!(src.contains("yield"), "the synthesized method must yield:\n{src}");
    assert!(src.contains("ensure"), "the ensure clause must carry over:\n{src}");
    assert!(
        src.contains("response.headers['X-Quota'] = '9'") || src.contains("response.headers[\"X-Quota\"] = \"9\""),
        "the header write must carry over:\n{src}"
    );
    // The dispatch is wrapped, guarded by `only: :create`.
    assert!(
        src.contains("if [:create].include?(action_name)"),
        "the only: guard must reach the dispatcher:\n{src}"
    );
}

#[test]
fn the_wrapped_action_runs_and_the_ensure_header_survives_a_raise() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/boom\", to: \"widgets#boom\"\n  get \"/calm\", to: \"widgets#calm\"\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  around_action(only: [:boom, :calm]) do |_controller, block|
    block.call
  ensure
    response.headers['X-Quota'] = '9'
  end

  def boom
    raise "kaboom"
  end

  def calm
    head :ok
  end
end
"#,
        )
        .run_ruby(
            r#"
require_relative "app/controllers/widgets_controller"

calm = WidgetsController.new
calm.process_action(:calm)
raise "the action itself must still run" unless calm.headers["X-Quota"] == "9"

controller = WidgetsController.new
raised = false
begin
  controller.process_action(:boom)
rescue RuntimeError => e
  raised = (e.message == "kaboom")
end
raise "the action's raise must propagate" unless raised
raise "ensure must still set the header: #{controller.headers.inspect}" unless controller.headers["X-Quota"] == "9"
puts "around ensure ran on both the calm and the raising path"
"#,
        )
        .assert_passes();
}

#[test]
fn an_if_guard_gates_the_block_form_around_filter() {
    let src = r#"class WidgetsController < ApplicationController
  around_action(if: :loud?) do |_controller, block|
    block.call
  end

  def index
  end

  private

  def loud?
    true
  end
end
"#;
    let (app, gaps) = build(src);
    assert!(!has_gap(&gaps, "around_action"), "{gaps:?}");
    let src = emitted_src(app);
    let dispatch = src.find("case action_name").map(|i| &src[..i]).unwrap_or(&src);
    assert!(
        dispatch.contains("if") && dispatch.contains("loud?"),
        "the if: guard must reach the dispatcher:\n{src}"
    );
}

#[test]
fn a_block_form_around_filter_keeps_declaration_order_with_surrounding_before_actions() {
    // `first`/`second` live on the PARENT, not on WidgetsController
    // itself — an own-private filter target instead inlines into the
    // action body (`inline_before_filters`), which would place both
    // reads inside `index`'s own body rather than the preamble this
    // test means to inspect.
    let files: Vec<(&str, &str)> = vec![
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  private\n\n  def first\n    @first = true\n  end\n\n  def second\n    @second = true\n  end\nend\n",
        ),
        (
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  before_action :first

  around_action(only: :index) do |_controller, block|
    block.call
  end

  before_action :second

  def index
  end
end
"#,
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    survey::activate();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let gaps = survey::drain();
    assert!(!has_gap(&gaps, "around_action"), "{gaps:?}");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_controllers(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("widgets_controller.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("widgets_controller.rb not emitted"));
    assert!(
        src.contains("__rh_around_"),
        "the block-form around_action must actually lower (not just leave the surrounding before_actions untouched):\n{src}"
    );
    let first = src.find("    first\n").expect("first before_action dispatched");
    let second = src.find("    second\n").expect("second before_action dispatched");
    let dispatch = src.find("case action_name").expect("case dispatch");
    assert!(
        first < second && second < dispatch,
        "declared order survives around the block-form filter:\n{src}"
    );
}

// ---------------------------------------------------------------------
// Refused: Commit 2 — a survey gap, and the call stays whole
// ---------------------------------------------------------------------

fn assert_refused(widgets_body: &str) -> (App, Vec<IngestError>) {
    let (app, gaps) = build(widgets_body);
    assert!(has_gap(&gaps, "around_action"), "{gaps:?}");
    let c = widgets_controller(&app);
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, block: Some(_), .. } if method.as_str() == "around_action"))),
        "a refused around_action must stay whole: {:?}",
        c.body
    );
    assert!(
        !c.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { filter, .. }
            if filter.kind == FilterKind::Around)),
        "a refused around_action must not half-expand: {:?}",
        c.body
    );
    (app, gaps)
}

#[test]
fn a_continuation_passed_along_is_refused() {
    assert_refused(
        r#"class WidgetsController < ApplicationController
  around_action do |_controller, block|
    somewhere_else(block)
  end

  def index
  end
end
"#,
    );
}

#[test]
fn a_stored_continuation_is_refused() {
    assert_refused(
        r#"class WidgetsController < ApplicationController
  around_action do |_controller, block|
    stashed = block
    stashed.call
  end

  def index
  end
end
"#,
    );
}

#[test]
fn a_continuation_called_with_arguments_is_refused() {
    assert_refused(
        r#"class WidgetsController < ApplicationController
  around_action do |_controller, block|
    block.call(1)
  end

  def index
  end
end
"#,
    );
}

#[test]
fn a_continuation_called_via_dot_paren_sugar_is_refused() {
    assert_refused(
        r#"class WidgetsController < ApplicationController
  around_action do |_controller, block|
    block.()
  end

  def index
  end
end
"#,
    );
}

#[test]
fn a_forwarded_continuation_block_pass_is_refused() {
    assert_refused(
        r#"class WidgetsController < ApplicationController
  around_action do |_controller, block|
    [1].each(&block)
  end

  def index
  end
end
"#,
    );
}

#[test]
fn three_block_parameters_are_refused() {
    assert_refused(
        r#"class WidgetsController < ApplicationController
  around_action do |_controller, block, extra|
    block.call
  end

  def index
  end
end
"#,
    );
}

#[test]
fn an_unrepresentable_option_is_refused() {
    assert_refused(
        r#"class WidgetsController < ApplicationController
  around_action(prepend: true) do |_controller, block|
    block.call
  end

  def index
  end
end
"#,
    );
}

#[test]
fn an_only_option_that_is_a_string_is_refused() {
    assert_refused(
        r#"class WidgetsController < ApplicationController
  around_action(only: 'index') do |_controller, block|
    block.call
  end

  def index
  end
end
"#,
    );
}

// ---------------------------------------------------------------------
// Byte-identical: a controller with no block-form around_action is
// unaffected by this whole lowering.
// ---------------------------------------------------------------------

#[test]
fn a_controller_with_no_block_form_around_is_emitted_unchanged() {
    let src = r#"class WidgetsController < ApplicationController
  around_action :track

  def index
  end

  private

  def track
    @before = true
    yield
    @after = true
  end
end
"#;
    let (app, gaps) = build(src);
    assert!(!has_gap(&gaps, "around_action"), "{gaps:?}");
    let c = widgets_controller(&app);
    assert!(
        !c.body.iter().any(|item| matches!(item, ControllerBodyItem::Action { action, .. }
            if action.name.as_str().starts_with("__rh_around_"))),
        "no synthesized method on an ordinary named around_action: {:?}",
        c.body
    );
    let src = emitted_src(app);
    assert!(!src.contains("__rh_around_"), "{src}");
    assert!(src.contains("self.track do"), "{src}");
}

// ---------------------------------------------------------------------
// Name collision across inheritance: a parent and a child controller
// each own block-form `around_action` must not synthesize the same
// method name (follow-up to rubys/roundhouse#784). Numbering restarted
// per controller, so a parent's `__rh_around_1` and a child's own
// `__rh_around_1` used to be the SAME method; through inheritance the
// child's definition overrode the parent's, so the parent's own around
// filter ran the CHILD's body instead of its own.
// ---------------------------------------------------------------------

#[test]
fn a_parent_and_child_controllers_block_form_arounds_synthesize_different_names() {
    let files: Vec<(&str, &str)> = vec![
        (
            "app/controllers/application_controller.rb",
            r#"class ApplicationController < ActionController::Base
  around_action do |_controller, block|
    response.headers['X-Order'] = (response.headers['X-Order'] || '') + 'P'
    block.call
  end
end
"#,
        ),
        (
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  around_action do |_controller, block|
    response.headers['X-Order'] = (response.headers['X-Order'] || '') + 'C'
    block.call
  end

  def index
  end
end
"#,
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let gaps = survey::drain();
    assert!(!has_gap(&gaps, "around_action"), "{gaps:?}");

    let synthesized_name = |controller_name: &str| -> String {
        let c = app
            .controllers
            .iter()
            .find(|c| c.name.0.as_str() == controller_name)
            .unwrap_or_else(|| panic!("{controller_name} ingested"));
        c.body
            .iter()
            .find_map(|item| match item {
                ControllerBodyItem::Action { action, .. }
                    if action.name.as_str().starts_with("__rh_around_") =>
                {
                    Some(action.name.as_str().to_string())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{controller_name} has a synthesized around method: {:?}", c.body))
    };
    let parent_name = synthesized_name("ApplicationController");
    let child_name = synthesized_name("WidgetsController");
    assert_ne!(
        parent_name, child_name,
        "a parent and child controller's own block-form arounds must not collide"
    );
    assert!(parent_name.contains("ApplicationController"), "{parent_name}");
    assert!(child_name.contains("WidgetsController"), "{child_name}");
}

// A namespaced controller and a flat one whose names underscore the same
// way (`Admin::WidgetsController` and `AdminWidgetsController` both give
// `admin_widgets_controller`) must not share a synthesized name either:
// when one inherits from the other, the collision is the same override.
#[test]
fn a_namespaced_controller_and_a_same_named_flat_parent_synthesize_different_names() {
    let files: Vec<(&str, &str)> = vec![
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/admin_widgets_controller.rb",
            r#"class AdminWidgetsController < ApplicationController
  around_action do |_controller, block|
    response.headers['X-Order'] = (response.headers['X-Order'] || '') + 'P'
    block.call
  end

  def index
  end
end
"#,
        ),
        (
            "app/controllers/admin/widgets_controller.rb",
            r#"module Admin
  class WidgetsController < ::AdminWidgetsController
    around_action do |_controller, block|
      response.headers['X-Order'] = (response.headers['X-Order'] || '') + 'C'
      block.call
    end

    def index
    end
  end
end
"#,
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :admin_widgets, only: [:index]\n  namespace :admin do\n    resources :widgets, only: [:index]\n  end\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let gaps = survey::drain();
    assert!(!has_gap(&gaps, "around_action"), "{gaps:?}");

    let synthesized_name = |controller_name: &str| -> String {
        let c = app
            .controllers
            .iter()
            .find(|c| c.name.0.as_str() == controller_name)
            .unwrap_or_else(|| panic!("{controller_name} ingested: {:?}", app.controllers.iter().map(|c| c.name.0.as_str()).collect::<Vec<_>>()));
        c.body
            .iter()
            .find_map(|item| match item {
                ControllerBodyItem::Action { action, .. }
                    if action.name.as_str().starts_with("__rh_around_") =>
                {
                    Some(action.name.as_str().to_string())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{controller_name} has a synthesized around method: {:?}", c.body))
    };
    let flat = synthesized_name("AdminWidgetsController");
    let namespaced = synthesized_name("Admin::WidgetsController");
    assert_ne!(flat, namespaced, "a namespaced controller must not collide with a same-underscored flat parent");
}

#[test]
fn a_parent_and_child_controllers_block_form_arounds_both_run_in_rails_order() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            r#"class ApplicationController < ActionController::Base
  around_action do |_controller, block|
    response.headers['X-Order'] = (response.headers['X-Order'] || '') + 'P'
    block.call
  end
end
"#,
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  around_action do |_controller, block|
    response.headers['X-Order'] = (response.headers['X-Order'] || '') + 'C'
    block.call
  end

  def index
    head :ok
  end
end
"#,
        )
        .run_ruby(
            r#"
require_relative "app/controllers/widgets_controller"

controller = WidgetsController.new
controller.process_action(:index)
order = controller.headers["X-Order"]
raise "both arounds must run, the parent wrapping the child: got #{order.inspect}" unless order == "PC"
puts "parent and child block-form arounds both ran in Rails order: #{order}"
"#,
        )
        .assert_passes();
}
