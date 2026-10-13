//! A concern's class-side filter macro whose body wraps a BLOCK-FORM
//! filter — `before_action(**kwargs) { |controller| ... }` — rather than
//! plain Symbol-target filter DSL.
//!
//! `expand_class_body_macros` (via `filters_from_macro_body` /
//! `filter_from_send`) only ever recognized a Symbol target, so a macro
//! like Mastodon's `vary_by` (reduced here to `stamp_header`)
//! stayed `Unknown` and reported two gaps: "class-body macro not
//! expanded" and "controller class-body macro not recognized". The
//! `before_action` it wraps was dropped outright, so the header it sets
//! never reached a response.
//!
//! The accepted idiom (one `**kwargs`-forwarding macro branding a value
//! as optionally-callable):
//!
//! ```ruby
//! module HeaderConcern
//!   extend ActiveSupport::Concern
//!   class_methods do
//!     def stamp_header(value, **kwargs)
//!       before_action(**kwargs) do |controller|
//!         response.headers['X-Stamp'] =
//!           value.respond_to?(:call) ? controller.instance_exec(&value) : value
//!       end
//!     end
//!   end
//! end
//! ```
//!
//! After `substitute_params` binds `value` to the call's actual argument
//! (a literal or a `Lambda`) and `kwargs` to its options Hash,
//! `value.respond_to?(:call)` is statically decidable:
//! `ingest::app::block_filter_from_macro_stmt` folds it, collapses the
//! resulting `If`, and — when the surviving branch is the
//! `instance_exec` one — inlines a zero-param, control-flow-free
//! lambda's body in its place. What's left is reconstructed in exactly
//! the shape `ingest::controller::lambda_filter_target` already reads
//! off a hand-written `before_action(...) { ... }`, so it rides the SAME
//! path through `build_filter_preamble` (taught, in that same change, to
//! accept a `**`-splatted options Hash alongside a bare one).
//!
//! A nil-returning lambda guard (`'Signature' if loud?` when `loud?` is
//! false) means the filter sets the header to nil, which the runtime's
//! `HeaderStore` drops (`header_value_ok?` refuses nil) — unlike
//! Rails/Puma, which would still send an empty-valued header. Accepted
//! as a pre-existing runtime gap this expansion does not introduce or
//! paper over.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{ControllerBodyItem, FilterKind};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};
use roundhouse::App;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The macro-body shape every "accepted" test starts from: a value
/// branded optionally-callable, its options forwarded verbatim.
const DEFAULT_BODY: &str = r#"      before_action(**kwargs) do |controller|
        response.headers['X-Stamp'] = value.respond_to?(:call) ? controller.instance_exec(&value) : value
      end"#;

fn concern_src(macro_body: &str) -> String {
    format!(
        "module HeaderConcern\n  extend ActiveSupport::Concern\n  class_methods do\n    def stamp_header(value, **kwargs)\n{macro_body}\n    end\n  end\nend\n"
    )
}

const APPLICATION_CONTROLLER: &str =
    "class ApplicationController < ActionController::Base\n  include HeaderConcern\nend\n";

/// Build the tree, ingest it under `survey`, and return the app plus
/// whatever gaps were recorded. `widgets_extra` is spliced into
/// WidgetsController's body below the macro call (private helper
/// methods the call's lambda needs, e.g. `loud?`).
fn build(macro_body: &str, call: &str, widgets_extra: &str) -> (App, Vec<IngestError>) {
    let concern = concern_src(macro_body);
    let widgets = format!(
        "class WidgetsController < ApplicationController\n  {call}\n\n  def index\n    head :ok\n  end\n{widgets_extra}\nend\n"
    );
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/header_concern.rb", concern),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER.to_string()),
        ("app/controllers/widgets_controller.rb", widgets),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
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
// Accepted shapes
// ---------------------------------------------------------------------

#[test]
fn string_literal_value_expands_to_a_before_action_setting_the_literal() {
    let (app, gaps) = build(DEFAULT_BODY, "stamp_header 'Accept-Language'", "");
    assert!(!has_gap(&gaps, "stamp_header"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("Accept-Language"), "{src}");
    assert!(src.contains("X-Stamp"), "{src}");
    assert!(!src.contains("respond_to?"), "a literal's respond_to? must fold away:\n{src}");
    assert!(!src.contains("instance_exec"), "the literal branch never calls instance_exec:\n{src}");
}

#[test]
fn zero_param_lambda_value_inlines_the_lambda_body() {
    let (app, gaps) = build(DEFAULT_BODY, "stamp_header(-> { 'Signature' })", "");
    assert!(!has_gap(&gaps, "stamp_header"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("Signature"), "{src}");
    assert!(!src.contains("respond_to?"), "{src}");
    assert!(!src.contains("instance_exec"), "the lambda's body must be INLINED, not called:\n{src}");
}

#[test]
fn lambda_with_only_scopes_the_filter_to_its_actions() {
    let (app, gaps) = build(
        DEFAULT_BODY,
        "stamp_header(-> { 'Signature' if loud? }, only: :index)",
        "\n  private\n\n  def loud?\n    true\n  end\n",
    );
    assert!(!has_gap(&gaps, "stamp_header"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("Signature"), "{src}");
    assert!(src.contains("loud?"), "{src}");
    assert!(
        src.contains("[:index].include?(action_name)"),
        "the macro's only: must scope the expanded filter:\n{src}"
    );
    assert!(!src.contains("respond_to?") && !src.contains("instance_exec"), "{src}");
}

#[test]
fn if_lambda_guard_on_the_macro_call_gates_the_filter() {
    let (app, gaps) = build(
        DEFAULT_BODY,
        "stamp_header('Accept-Language', if: -> { enabled? })",
        "\n  private\n\n  def enabled?\n    true\n  end\n",
    );
    assert!(!has_gap(&gaps, "stamp_header"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("Accept-Language"), "{src}");
    assert!(
        src.contains("enabled?"),
        "the if: lambda guard must reach the dispatcher:\n{src}"
    );
}

#[test]
fn unless_symbol_guard_on_the_macro_call_gates_the_filter() {
    let (app, gaps) = build(
        DEFAULT_BODY,
        "stamp_header('Accept-Language', unless: :quiet?)",
        "\n  private\n\n  def quiet?\n    false\n  end\n",
    );
    assert!(!has_gap(&gaps, "stamp_header"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("Accept-Language"), "{src}");
    assert!(
        src.contains("quiet?"),
        "the unless: symbol guard must reach the dispatcher:\n{src}"
    );
}

/// A macro body mixing a Symbol-target filter and a block-form one keeps
/// each statement's own shape, in declaration order.
#[test]
fn a_macro_body_mixing_a_symbol_and_a_block_filter_keeps_declaration_order() {
    let body = format!("      skip_before_action :verify_authenticity_token\n{DEFAULT_BODY}");
    let (app, gaps) = build(&body, "stamp_header 'Accept-Language'", "");
    assert!(!has_gap(&gaps, "stamp_header"), "{gaps:?}");
    let c = widgets_controller(&app);
    let skip_index = c.body.iter().position(|item| matches!(item,
        ControllerBodyItem::Filter { filter, .. }
            if filter.kind == FilterKind::Skip && filter.target.as_str() == "verify_authenticity_token"));
    let block_index = c.body.iter().position(|item| matches!(item,
        ControllerBodyItem::Unknown { expr, .. } if matches!(&*expr.node,
            ExprNode::Send { method, block: Some(_), .. } if method.as_str() == "before_action")));
    let (skip_index, block_index) = (
        skip_index.unwrap_or_else(|| panic!("skip_before_action missing: {:?}", c.body)),
        block_index.unwrap_or_else(|| panic!("expanded block filter missing: {:?}", c.body)),
    );
    assert!(
        skip_index < block_index,
        "declaration order must survive expansion: skip@{skip_index} block@{block_index}"
    );
}

// ---------------------------------------------------------------------
// Refusals — the gap stays, and the macro call stays whole
// ---------------------------------------------------------------------

fn assert_refused(macro_body: &str, call: &str) {
    let (app, gaps) = build(macro_body, call, "");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("stamp_header") && message.contains("not filter DSL"))),
        "{gaps:?}"
    );
    let c = widgets_controller(&app);
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "stamp_header"))),
        "a refused macro call must stay whole: {:?}",
        c.body
    );
    assert!(
        !c.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { .. })),
        "a refused macro must not half-expand: {:?}",
        c.body
    );
}

#[test]
fn a_block_declaring_two_params_is_refused() {
    let body = DEFAULT_BODY.replace("|controller|", "|controller, extra|");
    assert_refused(&body, "stamp_header('Accept-Language')");
}

#[test]
fn a_leftover_non_filter_statement_is_refused() {
    let body = format!("      puts :tracked\n{DEFAULT_BODY}");
    assert_refused(&body, "stamp_header('Accept-Language')");
}

#[test]
fn a_value_lambda_with_return_is_refused() {
    assert_refused(DEFAULT_BODY, "stamp_header(-> { return 'Signature' })");
}

#[test]
fn a_value_lambda_with_next_is_refused() {
    assert_refused(DEFAULT_BODY, "stamp_header(-> { next 'Signature' })");
}

#[test]
fn a_value_lambda_declaring_its_own_params_is_refused() {
    assert_refused(DEFAULT_BODY, "stamp_header(->(ctx) { 'Signature' })");
}

/// `only:`/`except:` a String isn't a shape `ir_symbol_list` reads as a
/// Symbol at all — it reads back as an EMPTY list, which would scope the
/// expanded filter to no actions instead of just `:index` (the opposite
/// of the all-or-nothing contract: silently wrong rather than refused).
#[test]
fn an_only_option_that_is_a_string_not_a_symbol_is_refused() {
    assert_refused(DEFAULT_BODY, "stamp_header('Accept-Language', only: 'index')");
}

/// `if:`/`unless:` a String is neither a Symbol method-name guard nor a
/// lambda `ir_lambda_body` can read — `lambda_filter_target` would read
/// it as no guard at all, running the filter unconditionally.
#[test]
fn an_if_option_that_is_a_string_not_a_symbol_or_lambda_is_refused() {
    assert_refused(DEFAULT_BODY, "stamp_header('Accept-Language', if: 'cond')");
}

/// `if:`/`unless:` an Array of symbols is neither shape either — same
/// silent-no-guard gap as the String case above.
#[test]
fn an_if_option_that_is_an_array_is_refused() {
    assert_refused(DEFAULT_BODY, "stamp_header('Accept-Language', if: [:a, :b])");
}

/// An option key `lambda_filter_target` doesn't recognize at all
/// (`prepend:`, meant for `prepend_before_action`-style ordering) is
/// silently ignored there rather than erroring — which would make the
/// expanded filter drop the ordering the macro call asked for without
/// any record of the gap.
#[test]
fn an_unrecognized_option_key_is_refused() {
    assert_refused(DEFAULT_BODY, "stamp_header('Accept-Language', prepend: true)");
}

/// The outer `|controller|` rewrite to `self` is a blind full-tree
/// rewrite with no notion of scope. A nested block that redeclares
/// `controller` as its OWN parameter shadows the outer one — its body's
/// `controller` means that inner parameter, not the filter block's —
/// so rewriting every `controller` read to `self` would reach inside
/// and corrupt it (`items.map { |controller| controller.to_s }` would
/// silently become `items.map { |controller| self.to_s }`).
#[test]
fn a_nested_block_param_shadowing_the_filter_block_param_is_refused() {
    let body = r#"      before_action(**kwargs) do |controller|
        response.headers['X-Stamp'] = value.map { |controller| controller.to_s }.join(',')
      end"#;
    assert_refused(body, "stamp_header([1, 2])");
}

/// Same shadowing hazard, a nested `->` lambda literal declaring its own
/// `controller` parameter instead of a nested block.
#[test]
fn a_nested_lambda_param_shadowing_the_filter_block_param_is_refused() {
    let body = r#"      before_action(**kwargs) do |controller|
        response.headers['X-Stamp'] = (->(controller) { controller.to_s }).call(1)
      end"#;
    assert_refused(body, "stamp_header(1)");
}

/// A nested block's semicolon-declared LOCAL (`|x; controller|`) shadows
/// exactly like a declared parameter, but the IR doesn't represent
/// block-locals at all — ingestion drops the declaration, keeping only
/// whatever the block's body does with it. What survives here is the
/// local's first write (`controller = x.to_s`), which is still a
/// same-named assignment at a nested depth and must be enough on its
/// own to refuse the macro.
#[test]
fn a_nested_block_local_shadowing_the_filter_block_param_is_refused() {
    let body = r#"      before_action(**kwargs) do |controller|
        response.headers['X-Stamp'] = value.map { |x; controller| controller = x.to_s; controller }.join(',')
      end"#;
    assert_refused(body, "stamp_header([1, 2])");
}

/// A plain reassignment of the filter block's OWN parameter, in the
/// SAME scope, with no nested block or lambda at all. Every read after
/// `controller = nil` is of whatever was assigned, not of the actual
/// controller the filter runs against — rewriting it to `self` would be
/// just as wrong as the shadowing cases above.
#[test]
fn a_reassignment_of_the_filter_block_param_itself_is_refused() {
    let body = r#"      before_action(**kwargs) do |controller|
        controller = nil
        response.headers['X-Stamp'] = value
      end"#;
    assert_refused(body, "stamp_header('Accept-Language')");
}

// ---------------------------------------------------------------------
// Byte-identical pin: an existing Symbol-target concern macro (the
// `allow_unauthenticated_access` shape from `class_body_macro_expansion.rs`)
// must emit identically — this change only adds a NEW fallback path that
// `filter_from_send` never reaches for a Symbol-target body.
// ---------------------------------------------------------------------

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

#[test]
fn symbol_target_concern_macros_still_expand_to_the_same_filter() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "app/controllers/concerns/authentication.rb",
            AUTHENTICATION.to_string(),
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include Authentication\nend\n"
                .to_string(),
        ),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  allow_unauthenticated_access only: %i[new create]\n\n  def new\n  end\n\n  def show\n  end\nend\n"
                .to_string(),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :things, only: [:new, :show]\nend\n"
                .to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let c = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "ThingsController")
        .expect("ThingsController");
    let skip = c
        .body
        .iter()
        .find_map(|item| match item {
            ControllerBodyItem::Filter { filter, .. } if filter.kind == FilterKind::Skip => Some(filter),
            _ => None,
        })
        .expect("skip filter expanded");
    assert_eq!(skip.target.as_str(), "require_authentication");
    assert_eq!(
        skip.only.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>(),
        vec!["new".to_string(), "create".to_string()]
    );
    assert!(skip.except.is_empty());
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_lowered_controllers(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("things_controller.rb"))
        .map(|f| f.content.clone())
        .expect("things_controller.rb emitted");
    // The skip is SCOPED (`only: [:new, :create]`), so it narrows the
    // inherited `before_action :require_authentication` to `except:
    // [:new, :create]` rather than removing it outright — `show` (not
    // in the skip's `only:`) must still run it, so its name must still
    // reach the dispatcher.
    assert!(
        src.contains("require_authentication"),
        "a Symbol-target macro's expansion must still reach the dispatcher:\n{src}"
    );
}

// ---------------------------------------------------------------------
// Runtime: the emitted Ruby target, booted and driven under CRuby.
// ---------------------------------------------------------------------

const HEADER_CONCERN_RUNTIME: &str = r#"module HeaderConcern
  extend ActiveSupport::Concern
  class_methods do
    def stamp_header(value, **kwargs)
      before_action(**kwargs) do |controller|
        response.headers['X-Stamp'] = value.respond_to?(:call) ? controller.instance_exec(&value) : value
      end
    end
  end
end
"#;

const RUNTIME_APPLICATION_CONTROLLER: &str =
    "class ApplicationController < ActionController::Base\n  include HeaderConcern\nend\n";

const RUNTIME_WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  stamp_header 'Accept-Language'

  def index
    head :ok
  end
end
"#;

const RUNTIME_GADGETS_CONTROLLER: &str = r#"class GadgetsController < ApplicationController
  stamp_header -> { 'Signature' if loud? }, only: :index

  def index
    head :ok
  end

  private

  def loud?
    params[:loud] == "1"
  end
end
"#;

const RUNTIME_ROUTES: &str = r#"Rails.application.routes.draw do
  resources :widgets, only: :index
  resources :gadgets, only: :index
end
"#;

const RUNTIME_SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";

#[test]
fn the_expanded_header_filter_runs_under_cruby() {
    let overlay = emit_and_run::empty_app()
        .write("app/controllers/concerns/header_concern.rb", HEADER_CONCERN_RUNTIME)
        .write("app/controllers/application_controller.rb", RUNTIME_APPLICATION_CONTROLLER)
        .write("app/controllers/widgets_controller.rb", RUNTIME_WIDGETS_CONTROLLER)
        .write("app/controllers/gadgets_controller.rb", RUNTIME_GADGETS_CONTROLLER)
        .write("config/routes.rb", RUNTIME_ROUTES)
        .write("db/schema.rb", RUNTIME_SCHEMA);
    overlay
        .run_ruby(
            r#"def get(path, query = "")
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new("") }
  status, headers, _body = Main.run_rack(env)
  [status, headers["x-stamp"]]
end

def expect(path, query, want)
  got = get(path, query)
  raise "GET #{path}?#{query} answered #{got.inspect}, want #{want.inspect}" unless got == want
end

# String literal: the header is always set.
expect("/widgets", "", [200, "Accept-Language"])
# A lambda guard that returns nil: HeaderStore drops it, so the header
# is absent (not an empty string — see this file's nil-drops note).
expect("/gadgets", "", [200, nil])
# The same lambda guard returning a value: the header is set.
expect("/gadgets", "loud=1", [200, "Signature"])
"#,
        )
        .assert_passes();
}

// ---------------------------------------------------------------------
// A NAMED keyword beside a forwarded `**kwargs` — `substitute_params`
// used to bind every macro parameter POSITIONALLY, keyword params
// included. `retire_endpoint '2022-11-14', only: [:index]` bound
// `sunset` to the whole `{only: [:index]}` options Hash (the second
// POSITIONAL slot) and `kwargs` to `{}` (nothing left to bind), losing
// the call's `only:` scope and emitting Ruby that doesn't even parse:
// `response.headers["Sunset"] = only: [:index] if only: [:index]`.
// Fixed to bind positionals from the call's positional args and NAMED
// keywords from its trailing keyword Hash by name, with `**kwargs`
// catching whatever is left over.
// ---------------------------------------------------------------------

const SUNSET_CONCERN: &str = r#"module SunsetConcern
  extend ActiveSupport::Concern
  class_methods do
    def retire_endpoint(date, sunset: nil, **kwargs)
      before_action(**kwargs) do |controller|
        response.headers['Deprecation'] = date
        response.headers['Sunset'] = sunset if sunset
      end
    end
  end
end
"#;

fn sunset_build(call: &str) -> (App, Vec<IngestError>) {
    let widgets = format!(
        "class WidgetsController < ApplicationController\n  {call}\n\n  def index\n    head :ok\n  end\nend\n"
    );
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/sunset_concern.rb", SUNSET_CONCERN.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include SunsetConcern\nend\n"
                .to_string(),
        ),
        ("app/controllers/widgets_controller.rb", widgets),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
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

fn assert_parses(src: &str) {
    let result = ruby_prism::parse(src.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{errors:?}\n{src}");
}

#[test]
fn a_named_keyword_beside_a_forwarded_kwrest_binds_by_name_not_position() {
    let (app, gaps) = sunset_build("retire_endpoint '2022-11-14', only: [:index]");
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert_parses(&src);
    assert!(
        src.contains("Deprecation") && src.contains("2022-11-14"),
        "the Deprecation value must be the date:\n{src}"
    );
    assert!(
        src.contains("[:index]"),
        "the call's only: scope must survive, not be lost to positional mis-binding:\n{src}"
    );
    assert!(
        !src.contains("only: [:index] if only: [:index]")
            && !src.contains("= only: [:index]"),
        "sunset must not be bound to the whole options Hash:\n{src}"
    );
    // `sunset` defaults to nil: either no Sunset header line at all, or
    // one that's guarded by a nil/false condition — never a bare write.
    let sunset_line = src.lines().find(|l| l.contains("Sunset"));
    if let Some(line) = sunset_line {
        assert!(
            line.contains("if") || line.contains("nil"),
            "an unguarded Sunset write means `sunset` bound to something truthy:\n{line}\nfull:\n{src}"
        );
    }
}

#[test]
fn a_positional_and_keyword_mix_binds_sunset_by_name_and_passes_if_through() {
    let (app, gaps) =
        sunset_build("retire_endpoint '2022-11-14', sunset: '2023-01-01', if: :alpha?");
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert_parses(&src);
    assert!(
        src.contains("2022-11-14"),
        "the Deprecation value must still be the date:\n{src}"
    );
    assert!(
        src.contains("2023-01-01"),
        "sunset must bind by NAME to its own keyword, not the if: guard:\n{src}"
    );
    assert!(
        src.contains("alpha?"),
        "the if: guard must reach the filter dispatcher:\n{src}"
    );
}

#[test]
fn a_missing_required_keyword_is_refused() {
    let concern = r#"module SunsetConcern
  extend ActiveSupport::Concern
  class_methods do
    def retire_endpoint(date, at:, **kwargs)
      before_action(**kwargs) do |controller|
        response.headers['At'] = at
      end
    end
  end
end
"#;
    let widgets =
        "class WidgetsController < ApplicationController\n  retire_endpoint '2022-11-14'\n\n  def index\n    head :ok\n  end\nend\n";
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/sunset_concern.rb", concern.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include SunsetConcern\nend\n"
                .to_string(),
        ),
        ("app/controllers/widgets_controller.rb", widgets.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest must not hard-fail");
    let gaps = survey::drain();
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("retire_endpoint") && message.contains("not filter DSL"))),
        "a required keyword the call omits must be refused, not guessed: {gaps:?}"
    );
    let c = widgets_controller(&app);
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "retire_endpoint"))),
        "a refused macro call must stay whole: {:?}",
        c.body
    );
    assert!(
        !c.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { .. })),
        "a refused macro must not half-expand: {:?}",
        c.body
    );
}

#[test]
fn an_unknown_keyword_with_no_kwrest_is_refused() {
    let concern = r#"module SunsetConcern
  extend ActiveSupport::Concern
  class_methods do
    def retire_endpoint(date, sunset: nil)
      before_action do |controller|
        response.headers['Deprecation'] = date
        response.headers['Sunset'] = sunset if sunset
      end
    end
  end
end
"#;
    let widgets = "class WidgetsController < ApplicationController\n  retire_endpoint '2022-11-14', sunset: '2023-01-01', bogus: true\n\n  def index\n    head :ok\n  end\nend\n";
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/sunset_concern.rb", concern.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include SunsetConcern\nend\n"
                .to_string(),
        ),
        ("app/controllers/widgets_controller.rb", widgets.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest must not hard-fail");
    let gaps = survey::drain();
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("retire_endpoint") && message.contains("not filter DSL"))),
        "an unclaimed keyword with no **rest to catch it must be refused: {gaps:?}"
    );
    let c = widgets_controller(&app);
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "retire_endpoint"))),
        "a refused macro call must stay whole: {:?}",
        c.body
    );
}

#[test]
fn a_positional_splat_this_binder_cannot_model_is_refused() {
    // `*dates`'s element count is unknown here; binding it to the
    // single `date` slot would be a guess (and would emit
    // `headers['Deprecation'] = *dates`, which isn't what Ruby assigns).
    let (app, gaps) = sunset_build("retire_endpoint(*dates, sunset: '2023-01-01')");
    assert!(
        has_gap(&gaps, "retire_endpoint"),
        "a positional splat of unknown arity must not be guessed into one param: {gaps:?}"
    );
    let c = widgets_controller(&app);
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "retire_endpoint"))),
        "a refused macro call must stay whole: {:?}",
        c.body
    );
}

#[test]
fn a_duplicate_keyword_key_binds_the_last_value_and_drops_earlier_duplicates() {
    // Ruby collapses a duplicate keyword key to its LAST value before the
    // call ever runs. `sunset` must bind to the last duplicate, and the
    // earlier one must not survive into `**kwargs` where a literal read
    // (`kwargs[:sunset]`) could fold back to the stale value.
    let concern = r#"module SunsetConcern
  extend ActiveSupport::Concern
  class_methods do
    def retire_endpoint(date, sunset: nil, **kwargs)
      before_action do |controller|
        response.headers['Deprecation'] = date
        response.headers['Sunset'] = sunset
        response.headers['SunsetRest'] = kwargs[:sunset]
      end
    end
  end
end
"#;
    let widgets = "class WidgetsController < ApplicationController\n  retire_endpoint '2022-11-14', sunset: '2023-01-01', sunset: '2024-06-01'\n\n  def index\n    head :ok\n  end\nend\n";
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/sunset_concern.rb", concern.to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include SunsetConcern\nend\n"
                .to_string(),
        ),
        ("app/controllers/widgets_controller.rb", widgets.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect();
    survey::activate();
    let app = ingest_app_from_tree(tree).expect("ingest must not hard-fail");
    let gaps = survey::drain();
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert_parses(&src);
    assert!(
        src.contains("2024-06-01"),
        "sunset must bind to the LAST duplicate, as Ruby does:\n{src}"
    );
    assert!(
        !src.contains("2023-01-01"),
        "an earlier duplicate must not leak into **kwargs, where a literal \
         `kwargs[:sunset]` read could fold back to it:\n{src}"
    );
}

#[test]
fn an_empty_keyword_rest_does_not_emit_a_bare_double_splat() {
    // No trailing keyword args at the call site means `kwargs`'s
    // leftover is empty: `before_action(**kwargs)` must not become
    // `before_action(**)`, which `ruby -c` refuses to parse. Checked on
    // the substituted macro body itself (the `Unknown` item ingest
    // leaves pre-lowering, same shape a hand-written block-form filter
    // would ingest as) — once `analyze_and_lower` consumes it into
    // `process_action`, the raw options Hash is gone from the final
    // source regardless of this bug, so that final text can't see it.
    let (app, gaps) = sunset_build("retire_endpoint '2022-11-14'");
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let c = widgets_controller(&app);
    let unknown = c
        .body
        .iter()
        .find_map(|item| match item {
            ControllerBodyItem::Unknown { expr, .. } => Some(expr),
            _ => None,
        })
        .expect("the block-form before_action must stay Unknown pre-lowering");
    let src = roundhouse::emit::ruby::emit_expr(unknown);
    assert_parses(&src);
    assert!(
        !src.contains("(**)"),
        "an empty keyword rest must not emit a bare, valueless double splat:\n{src}"
    );
}
