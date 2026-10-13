//! A concern's class-side filter macro whose body wraps a BLOCK-FORM
//! `around_action` — the shape behind Rails' own
//! `Api::RateLimitHeaders#with_rate_limit_headers`, reduced here to a
//! `QuotaHeaders#override_quota_headers`:
//!
//! ```ruby
//! module QuotaHeaders
//!   extend ActiveSupport::Concern
//!   class_methods do
//!     def override_quota_headers(method_name, options = {})
//!       around_action(only: method_name, if: :current_gadget) do |_controller, block|
//!         block.call
//!       ensure
//!         limiter = Quota.new(current_gadget, options)
//!         quota_headers = limiter.to_headers
//!         response.headers.merge!(quota_headers) unless response.headers['X-Quota-Remaining'].present? && quota_headers['X-Quota-Remaining'].to_i > response.headers['X-Quota-Remaining'].to_i
//!       end
//!     end
//!   end
//! end
//! ```
//!
//! `around_action`'s block form is excluded from `ingest::controller::
//! is_lambda_filter_macro` on purpose — a before/after block inlines as
//! a plain conditional statement, but an around has to run the rest of
//! the filter chain from inside its own body, which that path can't
//! express. Before this change, `expand_macro_filters` never tried the
//! one lowering that CAN express it
//! (`ingest::controller::around_block_filter`) for a macro's body: the
//! statement fell to the generic `NotFilterDsl` refusal and the whole
//! macro stayed `Unknown`, so `override_quota_headers`'s header never
//! reached a response — the "class-body macro not recognized" gap
//! rubys/roundhouse's own survey of Mastodon's
//! `Api::RateLimitHeaders`-shaped concerns kept reporting.
//!
//! `around_block_filter` is reused outright (not duplicated): the
//! macro's call-site arguments are bound to its parameters by
//! `substitute_params` exactly as any other filter macro's are (`only:
//! method_name` becomes `only: :create`, and the call's trailing
//! `family: :widgets` binds to the plain positional `options = {}`
//! parameter — no `**kwargs` needed, the same way Ruby itself binds a
//! bare trailing keyword Hash to an undeclared-keyword method), and the
//! substituted body is then handed to the SAME `around_block_filter`
//! that lowers a hand-written block-form `around_action` — same
//! synthesized method, same `yield` rewrite, same `ensure`/`rescue`
//! carry-over, same refusals.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{ControllerBodyItem, FilterKind};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};
use roundhouse::App;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const QUOTA_HEADERS_CONCERN: &str = r#"module QuotaHeaders
  extend ActiveSupport::Concern
  class_methods do
    def override_quota_headers(method_name, options = {})
      around_action(only: method_name, if: :current_gadget) do |_controller, block|
        block.call
      ensure
        limiter = Quota.new(current_gadget, options)
        quota_headers = limiter.to_headers
        response.headers.merge!(quota_headers) unless response.headers['X-Quota-Remaining'].present? && quota_headers['X-Quota-Remaining'].to_i > response.headers['X-Quota-Remaining'].to_i
      end
    end
  end
end
"#;

const QUOTA_MODEL: &str = r#"class Quota
  def initialize(gadget, options)
    @gadget = gadget
    @options = options
  end

  def to_headers
    { 'X-Quota-Remaining' => '5' }
  end
end
"#;

const APPLICATION_CONTROLLER: &str =
    "class ApplicationController < ActionController::Base\n  include QuotaHeaders\nend\n";

fn widgets_controller_src(call: &str) -> String {
    format!(
        "class WidgetsController < ApplicationController\n  {call}\n\n  def index\n    head :ok\n  end\n\n  def create\n    head :created\n  end\n\n  private\n\n  def current_gadget\n    @current_gadget\n  end\nend\n"
    )
}

fn build(call: &str) -> (App, Vec<IngestError>) {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/quota_headers.rb", QUOTA_HEADERS_CONCERN.to_string()),
        ("app/models/quota.rb", QUOTA_MODEL.to_string()),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER.to_string()),
        ("app/controllers/widgets_controller.rb", widgets_controller_src(call)),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :create]\nend\n"
                .to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect();
    survey::activate();
    let result = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    (result.expect("ingest must not hard-fail; an unexpanded macro is a survey gap"), gaps)
}

fn has_gap(gaps: &[IngestError], needle: &str) -> bool {
    gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains(needle)))
}

fn widgets_controller(app: &App) -> &roundhouse::dialect::Controller {
    app.controllers
        .iter()
        .find(|c| c.name.0.as_str() == "WidgetsController")
        .expect("WidgetsController ingested")
}

fn emitted_widgets(mut app: App) -> String {
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_lowered_controllers(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("widgets_controller.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("widgets_controller.rb not emitted"))
}

// ---------------------------------------------------------------------
// Accepted: the macro's block-form around_action lowers like a
// hand-written one.
// ---------------------------------------------------------------------

#[test]
fn the_macro_s_around_action_lowers_to_a_synthesized_method_with_only_if_yield_and_ensure() {
    let (app, gaps) = build("override_quota_headers :create, family: :widgets");
    assert!(!has_gap(&gaps, "override_quota_headers"), "{gaps:?}");
    let c = widgets_controller(&app);
    // No Unknown item survives for the macro call: it fully lowered.
    assert!(
        !c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "override_quota_headers"))),
        "a lowered macro call must not stay Unknown: {:?}",
        c.body
    );
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { filter, .. }
            if filter.kind == FilterKind::Around)),
        "a named Around filter must be registered: {:?}",
        c.body
    );
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Action { action, .. }
            if action.name.as_str().starts_with("__rh_around_"))),
        "a synthesized around method must be registered: {:?}",
        c.body
    );
    let src = emitted_widgets(app);
    assert!(src.contains("yield"), "the synthesized method must yield:\n{src}");
    assert!(src.contains("ensure"), "the ensure clause must carry over:\n{src}");
    assert!(src.contains("Quota.new"), "the ensure body must carry over verbatim:\n{src}");
    assert!(src.contains("merge!"), "the header merge must carry over:\n{src}");
    // The dispatch is wrapped, guarded by `only: :create` and `if: :current_gadget`.
    let dispatch = src.find("case action_name").map(|i| &src[..i]).unwrap_or(&src);
    assert!(
        dispatch.contains("[:create].include?(action_name)"),
        "the macro's only: method_name must scope the expanded filter to :create:\n{src}"
    );
    assert!(
        dispatch.contains("current_gadget"),
        "the macro's if: :current_gadget guard must reach the dispatcher:\n{src}"
    );
}

#[test]
fn the_options_hash_binds_the_call_s_trailing_keywords_with_no_kwargs_declared() {
    // `def override_quota_headers(method_name, options = {})` declares
    // no keyword parameter at all — Ruby binds a call's trailing
    // `family: :widgets` to the plain positional `options` slot. The
    // macro body reads `options` back inside `Quota.new(current_gadget,
    // options)`; that read must substitute to the call's actual Hash,
    // not an empty one.
    let (app, gaps) = build("override_quota_headers :create, family: :widgets");
    assert!(!has_gap(&gaps, "override_quota_headers"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(
        src.contains("family") && src.contains("widgets"),
        "the call's options Hash must substitute into the macro body:\n{src}"
    );
}

// ---------------------------------------------------------------------
// Refused: a `block` misuse the shared `around_block_filter` already
// refuses for a hand-written around_action refuses the WHOLE macro the
// same way, with exactly one gap.
// ---------------------------------------------------------------------

#[test]
fn a_continuation_called_with_arguments_is_refused_with_one_gap() {
    let concern = QUOTA_HEADERS_CONCERN.replace("block.call", "block.call(1)");
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/quota_headers.rb", concern),
        ("app/models/quota.rb", QUOTA_MODEL.to_string()),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER.to_string()),
        (
            "app/controllers/widgets_controller.rb",
            widgets_controller_src("override_quota_headers :create, family: :widgets"),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :create]\nend\n"
                .to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest must not hard-fail");
    let gaps = survey::drain();
    assert_eq!(gaps.len(), 1, "exactly one gap, all-or-nothing: {gaps:?}");
    assert!(
        has_gap(&gaps, "override_quota_headers")
            && has_gap(&gaps, "continuation parameter")
            && has_gap(&gaps, "bare `.call`"),
        "the specific around_block_filter reason must surface, not a generic one: {gaps:?}"
    );
    let c = widgets_controller(&app);
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "override_quota_headers"))),
        "a refused macro call must stay whole: {:?}",
        c.body
    );
    assert!(
        !c.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { .. })),
        "a refused macro must not half-expand: {:?}",
        c.body
    );
}

// ---------------------------------------------------------------------
// Runtime: the emitted Ruby target, booted and driven under CRuby.
// ---------------------------------------------------------------------

const RUNTIME_WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  override_quota_headers :create, family: :widgets

  def index
    head :ok
  end

  def create
    raise "kaboom" if params[:boom] == "1"
    head :created
  end

  private

  def current_gadget
    params[:gadget] == "1" ? true : nil
  end
end
"#;

const RUNTIME_APPLICATION_CONTROLLER: &str = r#"class ApplicationController < ActionController::Base
  include QuotaHeaders

  rescue_from StandardError do
    head :internal_server_error
  end
end
"#;

const RUNTIME_ROUTES: &str = r#"Rails.application.routes.draw do
  resources :widgets, only: [:index, :create]
end
"#;

const RUNTIME_SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";

#[test]
fn the_expanded_around_runs_under_cruby_and_survives_a_raise() {
    let overlay = emit_and_run::empty_app()
        .write("app/controllers/concerns/quota_headers.rb", QUOTA_HEADERS_CONCERN)
        .write("app/models/quota.rb", QUOTA_MODEL)
        .write("app/controllers/application_controller.rb", RUNTIME_APPLICATION_CONTROLLER)
        .write("app/controllers/widgets_controller.rb", RUNTIME_WIDGETS_CONTROLLER)
        .write("config/routes.rb", RUNTIME_ROUTES)
        .write("db/schema.rb", RUNTIME_SCHEMA);
    overlay
        .run_ruby(
            r##"
ActionController::Base.allow_forgery_protection = false

def post(path, query = "")
  env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new("") }
  status, headers, _body = Main.run_rack(env)
  [status, headers["x-quota-remaining"]]
end

def get(path, query = "")
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new("") }
  status, headers, _body = Main.run_rack(env)
  [status, headers["x-quota-remaining"]]
end

def expect(got, want, label)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end

# POST /widgets with current_gadget set: the around's ensure sets the header.
expect(post("/widgets", "gadget=1"), [201, "5"], "create with current_gadget")

# GET /widgets (index): only: :create means the filter never wraps it.
expect(get("/widgets", "gadget=1"), [200, nil], "index is not wrapped")

# POST /widgets with no current_gadget: if: :current_gadget gates the whole filter off.
expect(post("/widgets", ""), [201, nil], "create without current_gadget")

# POST /widgets that raises: ensure still runs before rescue_from answers.
expect(post("/widgets", "gadget=1&boom=1"), [500, "5"], "create that raises")

puts "the macro-expanded around ran its ensure on every path, including the raise"
"##,
        )
        .assert_passes();
}

// ---------------------------------------------------------------------
// Byte-identical: a controller with no macro-wrapped around_action is
// unaffected. Pinned against the plain Symbol-target macro expansion
// already covered by `tests/concern_block_filter_macros.rs`.
// ---------------------------------------------------------------------

#[test]
fn a_controller_with_no_around_wrapping_macro_is_emitted_unchanged() {
    let concern = r#"module HeaderConcern
  extend ActiveSupport::Concern
  class_methods do
    def stamp_header(value)
      before_action do
        response.headers['X-Stamp'] = value
      end
    end
  end
end
"#;
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/header_concern.rb", concern.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include HeaderConcern\nend\n"
                .to_string(),
        ),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  stamp_header 'Accept-Language'\n\n  def index\n    head :ok\n  end\nend\n"
                .to_string(),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let gaps = survey::drain();
    assert!(!has_gap(&gaps, "stamp_header"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(!src.contains("__rh_around_"), "no around synthesis for a before_action-only macro:\n{src}");
}
