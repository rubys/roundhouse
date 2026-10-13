//! A `deprecate_api`-shaped concern macro: a block-form filter whose
//! macro body computes LOCALS before the filter statement itself —
//! `stamp = "@#{date.to_datetime.to_i}"`, `sunset =
//! sunset&.to_date&.httpdate` (reassigning, and so shadowing, the
//! `sunset:` parameter). Reduced here to `SunsetConcern#retire_endpoint`,
//! generic names for Mastodon's `deprecate_api`.
//!
//! Before this change, `expand_macro_filters` (`src/ingest/app.rs`)
//! refused the first statement outright — `stamp = …` is an `Assign`,
//! neither a Symbol-target filter (`filter_from_send`) nor a block-form
//! one (`block_filter_from_macro_stmt`) — so the macro never expanded at
//! all.
//!
//! What's new: `expand_macro_filters` folds a PREFIX of plain-local
//! assignments to literals at compile time
//! (`fold_local_value`/`const_fold_expr`) before trying the remaining
//! statement as a filter — refusing, same as any other statement that
//! isn't filter DSL, the moment a local can't be reduced to a literal.
//!
//! Also here: `next` inside a block filter's body. Unmodified, it is
//! copied verbatim into `process_action` — a plain method, not a block
//! or a loop — which `ruby -c` rejects ("Invalid next"). A `next
//! unless`/`next if` guard is now restructured into the equivalent
//! `if`/`unless` wrapping the rest of the block, in
//! `ingest::controller::lambda_filter_target` — the one resolver a
//! macro-expanded block filter and a hand-written one both go through.
//!
//! And the OTel tail: `OpenTelemetry::Trace.current_span`. Without a
//! stub, that constant simply does not exist on either target, so a
//! deprecated endpoint 500s the moment its filter reaches it.
//! `runtime/ruby/open_telemetry_facade.rb` gives it a non-recording
//! span — exactly `opentelemetry-api`'s own behavior with no SDK
//! installed — permanently, on every target, since nothing here ever
//! stands aside for a real gem.
//!
//! The full `retire_endpoint` shape at the bottom of this file —
//! locals, the `next unless` OTel guard, and the stub together — is the
//! brief's actual macro; everything above builds up to it one feature
//! at a time.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::ControllerBodyItem;
use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};
use roundhouse::App;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The macro-body shape every "accepted"/"refused" ingest-level test
/// starts from — the shape in this crate's brief, generic names.
const DEFAULT_MACRO_BODY: &str = r#"      stamp = "@#{date.to_datetime.to_i}"
      sunset = sunset&.to_date&.httpdate
      before_action(**kwargs) do
        response.headers['Deprecation'] = stamp
        response.headers['Sunset'] = sunset if sunset
      end"#;

fn concern_src(macro_body: &str) -> String {
    format!(
        "module SunsetConcern\n  extend ActiveSupport::Concern\n\n  class_methods do\n    def retire_endpoint(date, sunset: nil, **kwargs)\n{macro_body}\n    end\n  end\nend\n"
    )
}

const APPLICATION_CONTROLLER: &str =
    "class ApplicationController < ActionController::Base\n  include SunsetConcern\nend\n";

/// Build the tree, ingest it under `survey`, and return the app plus
/// whatever gaps were recorded. `widgets_extra` is spliced into
/// WidgetsController's body below the macro call (private helper
/// methods an `if:`/`unless:` lambda needs).
fn build(macro_body: &str, call: &str, widgets_extra: &str) -> (App, Vec<IngestError>) {
    let concern = concern_src(macro_body);
    let widgets = format!(
        "class WidgetsController < ApplicationController\n  {call}\n\n  def index\n    head :ok\n  end\n\n  def create\n    head :created\n  end\n{widgets_extra}\nend\n"
    );
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/sunset_concern.rb", concern),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER.to_string()),
        ("app/controllers/widgets_controller.rb", widgets),
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
// Accepted shapes
// ---------------------------------------------------------------------

/// `'2022-11-14'.to_datetime.to_i` folds to the literal Integer; `only:`
/// scopes the expanded filter; with no `sunset:` the Sunset header line
/// does not survive (folded away by `sunset = nil&.to_date&.httpdate`
/// and the `… if sunset` guard it feeds, both literal-nil).
#[test]
fn a_literal_date_folds_the_stamp_and_only_scopes_the_filter() {
    let (app, gaps) = build(DEFAULT_MACRO_BODY, "retire_endpoint '2022-11-14', only: [:index]", "");
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("@1668384000"), "the date must fold to the literal stamp:\n{src}");
    assert!(!src.contains("to_datetime") && !src.contains("to_i"), "folded away, not computed at runtime:\n{src}");
    assert!(
        src.contains("[:index].include?(action_name)"),
        "only: must scope the expanded filter:\n{src}"
    );
    assert!(!src.contains("Sunset"), "no sunset: given — the header line must not survive:\n{src}");
}

#[test]
fn an_if_symbol_guard_on_the_macro_call_reaches_the_dispatcher() {
    let (app, gaps) = build(
        DEFAULT_MACRO_BODY,
        "retire_endpoint '2021-05-16', if: :alpha?",
        "\n  private\n\n  def alpha?\n    true\n  end\n",
    );
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("@1621123200"), "{src}");
    assert!(src.contains("alpha?"), "the if: Symbol guard must reach the dispatcher:\n{src}");
}

#[test]
fn an_if_lambda_guard_on_the_macro_call_reaches_the_dispatcher() {
    let (app, gaps) = build(
        DEFAULT_MACRO_BODY,
        "retire_endpoint '2026-06-10', if: -> { request.path == '/w' }",
        "",
    );
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("@1781049600"), "{src}");
    assert!(src.contains("request.path"), "the if: lambda guard must reach the dispatcher:\n{src}");
}

#[test]
fn a_literal_sunset_folds_to_the_httpdate_string() {
    let (app, gaps) = build(
        DEFAULT_MACRO_BODY,
        "retire_endpoint '2022-11-14', sunset: '2026-10-01', only: [:index]",
        "",
    );
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("@1668384000"), "{src}");
    assert!(
        src.contains("Thu, 01 Oct 2026 00:00:00 GMT"),
        "sunset: must fold to Date#httpdate's RFC 2822 string:\n{src}"
    );
    assert!(!src.contains("to_date") && !src.contains("httpdate"), "folded away, not computed at runtime:\n{src}");
}

// ---------------------------------------------------------------------
// A plain new local (never a macro parameter) must bind BY NAME only —
// `by_span` is for a local that REASSIGNS a parameter, not for one that
// merely reads one mid-expression.
// ---------------------------------------------------------------------

/// `stamp` folds `date.to_datetime.to_i`, but never reassigns `date`
/// itself — `date` is read again, untouched, right beside it. Before the
/// fix, `expand_macro_filters` filled `by_span` for every folded local
/// (not only one that reassigns a parameter), so the span `date`'s
/// literal clone carries — shared by EVERY read of `date` in the macro
/// body, since `substitute_params` substitutes the same parameter value
/// everywhere — got mapped to `stamp`'s folded value. The later, direct
/// `response.headers['X-Date'] = date` was then wrongly rewritten to
/// `stamp`'s own value instead of keeping `date`'s.
const STAMP_AND_DATE_READ_MACRO_BODY: &str = r#"      stamp = "@#{date.to_datetime.to_i}"
      before_action(**kwargs) do
        response.headers['X-Date'] = date
        response.headers['Deprecation'] = stamp
      end"#;

#[test]
fn a_plain_local_does_not_shadow_a_later_read_of_the_parameter_it_reads() {
    let (app, gaps) =
        build(STAMP_AND_DATE_READ_MACRO_BODY, "retire_endpoint '2022-11-14', only: [:index]", "");
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("@1668384000"), "stamp must still fold to its own literal:\n{src}");
    assert!(
        src.contains("2022-11-14"),
        "a later, direct read of `date` must keep date's own literal, not stamp's:\n{src}"
    );
}

fn build_custom(concern: &str, call: &str) -> (App, Vec<IngestError>) {
    let widgets = format!(
        "class WidgetsController < ApplicationController\n  {call}\n\n  def index\n    head :ok\n  end\nend\n"
    );
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/concerns/sunset_concern.rb", concern.to_string()),
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

/// Same shape, a DEFAULTED parameter this time: `flag` is never supplied
/// at the call site, so it binds to its own default `'on'` — a literal
/// clone of the SAME span wherever the macro body reads `flag`. `label`
/// folds `"#{flag}!"` without reassigning `flag` itself; the later,
/// direct `response.headers['X-Flag'] = flag` must still read `flag`'s
/// own bound value, not `label`'s.
const FLAG_LABEL_CONCERN: &str = "module SunsetConcern\n  extend ActiveSupport::Concern\n\n  class_methods do\n    def m(flag = 'on', **kw)\n      label = \"#{flag}!\"\n      before_action(**kw) do\n        response.headers['X-Flag'] = flag\n        response.headers['X-Label'] = label\n      end\n    end\n  end\nend\n";

#[test]
fn a_plain_local_does_not_shadow_a_later_read_of_a_defaulted_parameter() {
    let (app, gaps) = build_custom(FLAG_LABEL_CONCERN, "m");
    assert!(gaps.is_empty(), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("\"on!\""), "label must still fold to its own literal:\n{src}");
    assert!(
        src.contains("\"on\""),
        "a later, direct read of `flag` must keep flag's own default, not label's:\n{src}"
    );
}

// ---------------------------------------------------------------------
// Refusals — the gap stays, and the macro call stays whole
// ---------------------------------------------------------------------

fn assert_refused(macro_body: &str, call: &str) {
    let (app, gaps) = build(macro_body, call, "");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("retire_endpoint") && message.contains("not filter DSL"))),
        "{gaps:?}"
    );
    let c = widgets_controller(&app);
    assert!(
        c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, roundhouse::expr::ExprNode::Send { method, .. } if method.as_str() == "retire_endpoint"))),
        "a refused macro call must stay whole: {:?}",
        c.body
    );
    assert!(
        !c.body.iter().any(|item| matches!(item, ControllerBodyItem::Filter { .. })),
        "a refused macro must not half-expand: {:?}",
        c.body
    );
}

/// The macro's primary date argument, not a literal: `stamp = …` cannot
/// fold (`Date.today` is not a shape `const_fold_expr` understands), so
/// the whole macro is refused rather than leave a free variable.
#[test]
fn a_non_literal_date_is_refused() {
    assert_refused(DEFAULT_MACRO_BODY, "retire_endpoint Date.today, only: [:index]");
}

/// The `sunset:` keyword reassigned from a non-literal value: `date`
/// folds fine (the `stamp` local is bound), but `sunset = sunset&.to_date
/// &.httpdate` cannot — `sunset`'s bound value is `Date.today`, not a
/// literal string `const_fold_expr` can run `.to_date` against. Covers
/// both "a local that can't be folded" and "a param reassigned to
/// something that can't be folded" — the same statement is both.
#[test]
fn a_reassigned_param_that_cannot_fold_is_refused() {
    assert_refused(
        DEFAULT_MACRO_BODY,
        "retire_endpoint '2022-11-14', sunset: Date.today, only: [:index]",
    );
}

/// A statement AFTER the filter — even one that would fold fine on its
/// own — refuses the whole macro: `expand_macro_filters` only folds
/// locals in the PREFIX, ahead of the one filter statement it then
/// recognizes.
#[test]
fn a_statement_after_the_filter_is_refused() {
    let body = format!("{DEFAULT_MACRO_BODY}\n      logged = true");
    assert_refused(&body, "retire_endpoint '2022-11-14', only: [:index]");
}

// ---------------------------------------------------------------------
// Runtime: the emitted Ruby target, booted and driven under CRuby.
// ---------------------------------------------------------------------

const RUNTIME_SUNSET_CONCERN: &str = r#"module SunsetConcern
  extend ActiveSupport::Concern

  class_methods do
    def retire_endpoint(date, sunset: nil, **kwargs)
      stamp = "@#{date.to_datetime.to_i}"
      sunset = sunset&.to_date&.httpdate
      before_action(**kwargs) do
        response.headers['Deprecation'] = stamp
        response.headers['Sunset'] = sunset if sunset
      end
    end
  end
end
"#;

const RUNTIME_APPLICATION_CONTROLLER: &str =
    "class ApplicationController < ActionController::Base\n  include SunsetConcern\nend\n";

const RUNTIME_WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  retire_endpoint '2022-11-14', sunset: '2026-10-01', only: [:index]

  def index
    head :ok
  end

  def show
    head :ok
  end
end
"#;

const RUNTIME_ROUTES: &str = r#"Rails.application.routes.draw do
  resources :widgets, only: [:index, :show]
end
"#;

const RUNTIME_SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";

/// `GET /widgets` (scoped `only: [:index]`) answers 200 with both the
/// folded `Deprecation` stamp and the folded `Sunset` httpdate; `GET
/// /widgets/1` (`show`, not in `only:`) carries neither.
#[test]
fn the_expanded_locals_fold_and_run_under_cruby() {
    let overlay = emit_and_run::empty_app()
        .write("app/controllers/concerns/sunset_concern.rb", RUNTIME_SUNSET_CONCERN)
        .write("app/controllers/application_controller.rb", RUNTIME_APPLICATION_CONTROLLER)
        .write("app/controllers/widgets_controller.rb", RUNTIME_WIDGETS_CONTROLLER)
        .write("config/routes.rb", RUNTIME_ROUTES)
        .write("db/schema.rb", RUNTIME_SCHEMA);
    overlay
        .run_ruby(
            r#"def get(path)
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new("") }
  status, headers, _body = Main.run_rack(env)
  [status, headers["deprecation"], headers["sunset"]]
end

index = get("/widgets")
raise "GET /widgets answered #{index.inspect}" unless index == [200, "@1668384000", "Thu, 01 Oct 2026 00:00:00 GMT"]

show = get("/widgets/1")
raise "GET /widgets/1 answered #{show.inspect}, want no Deprecation/Sunset" unless show == [200, nil, nil]
"#,
        )
        .assert_passes();
}

// ---------------------------------------------------------------------
// `next` inside a block filter — restructured, not left verbatim.
// ---------------------------------------------------------------------

/// A macro body whose block filter holds a `next unless` AFTER the
/// locals this file's first commit already folds — proves the two
/// features compose: `stamp` is still folded, and the guard is still
/// restructured.
const NEXT_GUARD_MACRO_BODY: &str = r#"      stamp = "@#{date.to_datetime.to_i}"
      before_action(**kwargs) do
        response.headers['Deprecation'] = stamp
        next unless stamp
        response.headers['X-Extra'] = '1'
      end"#;

#[test]
fn a_macro_expanded_next_unless_guard_is_restructured_to_an_if() {
    let (app, gaps) = build(NEXT_GUARD_MACRO_BODY, "retire_endpoint '2022-11-14', only: [:index]", "");
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(!src.contains("next"), "the next-unless guard must be restructured to an if:\n{src}");
    assert!(src.contains("X-Extra"), "{src}");
}

const NEXT_GUARD_RUNTIME_CONCERN: &str = r#"module SunsetConcern
  extend ActiveSupport::Concern

  class_methods do
    def retire_endpoint(date, sunset: nil, **kwargs)
      stamp = "@#{date.to_datetime.to_i}"
      before_action(**kwargs) do
        response.headers['Deprecation'] = stamp
        next unless query_string.include?('extra')
        response.headers['X-Extra'] = '1'
      end
    end
  end
end
"#;

const NEXT_GUARD_RUNTIME_WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  retire_endpoint '2022-11-14', only: [:index]

  def index
    head :ok
  end
end
"#;

const NEXT_GUARD_RUNTIME_ROUTES: &str = r#"Rails.application.routes.draw do
  resources :widgets, only: [:index]
end
"#;

#[test]
fn a_macro_expanded_next_unless_guard_runs_under_cruby() {
    let overlay = emit_and_run::empty_app()
        .write("app/controllers/concerns/sunset_concern.rb", NEXT_GUARD_RUNTIME_CONCERN)
        .write("app/controllers/application_controller.rb", RUNTIME_APPLICATION_CONTROLLER)
        .write("app/controllers/widgets_controller.rb", NEXT_GUARD_RUNTIME_WIDGETS_CONTROLLER)
        .write("config/routes.rb", NEXT_GUARD_RUNTIME_ROUTES)
        .write("db/schema.rb", RUNTIME_SCHEMA);
    overlay
        .run_ruby(
            r#"def get(path, query = "")
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new("") }
  status, headers, _body = Main.run_rack(env)
  [status, headers["deprecation"], headers["x-extra"]]
end

plain = get("/widgets")
raise "no extra query must not set X-Extra: #{plain.inspect}" unless plain == [200, "@1668384000", nil]

extra = get("/widgets", "extra=1")
raise "extra=1 must set X-Extra: #{extra.inspect}" unless extra == [200, "@1668384000", "1"]
"#,
        )
        .assert_passes();
}

const NEXT_APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

/// A HAND-WRITTEN block filter (no concern macro involved) whose body
/// holds `next unless` — `ingest::controller::lambda_filter_target`
/// restructures this the same way it does a macro-expanded one.
/// `run_ruby` requiring `main.rb` is itself the `ruby -c` claim: a bare
/// `next` left in `process_action` would be a `SyntaxError` there,
/// which `assert_passes` reports as a failed run, not a clean 200.
const NEXT_WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  before_action only: [:index] do
    next unless query_string.include?('loud')
    response.headers['X-Loud'] = '1'
  end

  def index
    head :ok
  end
end
"#;

#[test]
fn a_hand_written_next_unless_guard_emits_valid_ruby_and_runs_under_cruby() {
    let overlay = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", NEXT_APPLICATION_CONTROLLER)
        .write("app/controllers/widgets_controller.rb", NEXT_WIDGETS_CONTROLLER)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n",
        )
        .write("db/schema.rb", RUNTIME_SCHEMA);
    overlay
        .run_ruby(
            r#"def get(path, query = "")
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new("") }
  status, headers, _body = Main.run_rack(env)
  [status, headers["x-loud"]]
end

raise "no loud query must not set the header: #{get("/widgets").inspect}" unless get("/widgets") == [200, nil]
raise "loud=1 must set the header: #{get("/widgets", "loud=1").inspect}" unless get("/widgets", "loud=1") == [200, "1"]
"#,
        )
        .assert_passes();
}

// ---------------------------------------------------------------------
// A `next` that `restructure_next_in_block` genuinely refuses — a
// located diagnostic now, not a silent drop. `next 1 if admin?` carries
// a VALUE, so it is not the bare `next` `next_guard_cond` recognizes;
// the whole block filter is refused rather than copy the `next` verbatim
// into `process_action` (invalid there) or drop the filter with no
// trace.
// ---------------------------------------------------------------------

const UNRESTRUCTURABLE_NEXT_WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  before_action only: [:index] do
    next 1 if admin?
    response.headers['X-Flag'] = '1'
  end

  def index
    head :ok
  end
end
"#;

fn next_refusal_tree(widgets: &str) -> HashMap<PathBuf, Vec<u8>> {
    [
        ("app/controllers/application_controller.rb", NEXT_APPLICATION_CONTROLLER.to_string()),
        ("app/controllers/widgets_controller.rb", widgets.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect()
}

#[test]
fn a_next_carrying_a_value_refuses_the_block_filter_in_strict_mode() {
    let tree = next_refusal_tree(UNRESTRUCTURABLE_NEXT_WIDGETS_CONTROLLER);
    let err =
        ingest_app_from_tree(tree).expect_err("strict mode must refuse a next it can't restructure");
    assert!(
        matches!(&err, IngestError::Unsupported { message, .. }
            if message.contains("next") && message.contains("before_action")),
        "{err:?}"
    );
}

#[test]
fn a_next_carrying_a_value_is_a_survey_gap_not_a_silent_drop() {
    let tree = next_refusal_tree(UNRESTRUCTURABLE_NEXT_WIDGETS_CONTROLLER);
    survey::activate();
    let result = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    result.expect("survey mode must not hard-fail; a refused next is a gap, not an abort");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("next") && message.contains("before_action"))),
        "{gaps:?}"
    );
}

// ---------------------------------------------------------------------
// The same refusal, MACRO-EXPANDED: a concern macro whose own block
// filter holds the same unrestructurable `next` — `block_filter_from_
// macro_stmt` (`src/ingest/app.rs`), not `lambda_filter_target`
// directly, reconstructs this shape during `expand_class_body_macros`.
// ---------------------------------------------------------------------

const MACRO_NEXT_REFUSAL_CONCERN: &str = "module SunsetConcern\n  extend ActiveSupport::Concern\n\n  class_methods do\n    def retire(**kw)\n      before_action(**kw) do\n        next 1 if admin?\n        response.headers['X'] = '1'\n      end\n    end\n  end\nend\n";

fn macro_next_refusal_tree(call: &str) -> HashMap<PathBuf, Vec<u8>> {
    let widgets = format!(
        "class WidgetsController < ApplicationController\n  {call}\n\n  def index\n    head :ok\n  end\nend\n"
    );
    [
        ("app/controllers/concerns/sunset_concern.rb", MACRO_NEXT_REFUSAL_CONCERN.to_string()),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER.to_string()),
        ("app/controllers/widgets_controller.rb", widgets),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n".to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
    .collect()
}

#[test]
fn a_macro_expanded_next_carrying_a_value_refuses_in_strict_mode() {
    let tree = macro_next_refusal_tree("retire only: [:index]");
    let err = ingest_app_from_tree(tree)
        .expect_err("strict mode must refuse a macro-expanded next it can't restructure");
    assert!(
        matches!(&err, IngestError::Unsupported { message, .. }
            if message.contains("next") && message.contains("retire")),
        "{err:?}"
    );
}

#[test]
fn a_macro_expanded_next_carrying_a_value_is_a_survey_gap_not_a_silent_drop() {
    let tree = macro_next_refusal_tree("retire only: [:index]");
    survey::activate();
    let result = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    result.expect("survey mode must not hard-fail; a refused next is a gap, not an abort");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("next") && message.contains("retire"))),
        "{gaps:?}"
    );
}

// ---------------------------------------------------------------------
// The OTel stub, standalone — no macro, no next, just the constant.
// ---------------------------------------------------------------------

const OTEL_APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";

const OTEL_WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  before_action do
    span = OpenTelemetry::Trace.current_span
    response.headers['X-Recording'] = span.recording?.to_s
    span.set_attribute('probe', true)
  end

  def index
    head :ok
  end
end
"#;

/// Without the stub this body's `before_action` would 500 every
/// request on `undefined constant OpenTelemetry` — a bare `NameError`,
/// not something a `rescue StandardError` in the app could even catch.
#[test]
fn open_telemetry_current_span_is_non_recording_and_does_not_raise() {
    let overlay = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", OTEL_APPLICATION_CONTROLLER)
        .write("app/controllers/widgets_controller.rb", OTEL_WIDGETS_CONTROLLER)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n",
        )
        .write("db/schema.rb", RUNTIME_SCHEMA);
    overlay
        .run_ruby(
            r#"env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets", "QUERY_STRING" => "", "rack.input" => StringIO.new("") }
status, headers, _body = Main.run_rack(env)
raise "GET /widgets answered #{[status, headers["x-recording"]].inspect}" unless [status, headers["x-recording"]] == [200, "false"]
"#,
        )
        .assert_passes();
}

// ---------------------------------------------------------------------
// End to end: locals, the next-unless OTel guard, and the stub together
// — the brief's actual `retire_endpoint` macro, unreduced.
// ---------------------------------------------------------------------

const FULL_MACRO_BODY: &str = r#"      stamp = "@#{date.to_datetime.to_i}"
      sunset = sunset&.to_date&.httpdate
      before_action(**kwargs) do
        response.headers['Deprecation'] = stamp
        response.headers['Sunset'] = sunset if sunset
        span = OpenTelemetry::Trace.current_span
        next unless span&.recording?
        span.set_attribute('app.endpoint.deprecated', true)
      end"#;

#[test]
fn the_full_macro_folds_locals_restructures_next_and_reaches_the_otel_tail() {
    let (app, gaps) = build(FULL_MACRO_BODY, "retire_endpoint '2022-11-14', only: [:index]", "");
    assert!(!has_gap(&gaps, "retire_endpoint"), "{gaps:?}");
    let src = emitted_widgets(app);
    assert!(src.contains("@1668384000"), "{src}");
    assert!(!src.contains("next"), "the next-unless guard must be restructured to an if:\n{src}");
    assert!(src.contains("recording?"), "{src}");
}

const FULL_RUNTIME_SUNSET_CONCERN: &str = r#"module SunsetConcern
  extend ActiveSupport::Concern

  class_methods do
    def retire_endpoint(date, sunset: nil, **kwargs)
      stamp = "@#{date.to_datetime.to_i}"
      sunset = sunset&.to_date&.httpdate
      before_action(**kwargs) do
        response.headers['Deprecation'] = stamp
        response.headers['Sunset'] = sunset if sunset
        span = OpenTelemetry::Trace.current_span
        next unless span&.recording?
        span.set_attribute('app.endpoint.deprecated', true)
      end
    end
  end
end
"#;

const FULL_RUNTIME_WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  retire_endpoint '2022-11-14', sunset: '2026-10-01', only: [:index]

  def index
    head :ok
  end

  def show
    head :ok
  end
end
"#;

const FULL_RUNTIME_ROUTES: &str = r#"Rails.application.routes.draw do
  resources :widgets, only: [:index, :show]
end
"#;

/// `GET /widgets` (scoped `only: [:index]`) answers 200 with both the
/// folded `Deprecation` stamp and the folded `Sunset` httpdate; `GET
/// /widgets/1` (`show`, not in `only:`) carries neither. Either
/// response at all — rather than a 500 — is also the OTel-tail claim: a
/// non-recording `current_span` that raised would 500 every request
/// the filter runs on.
#[test]
fn the_full_macro_runs_under_cruby() {
    let overlay = emit_and_run::empty_app()
        .write("app/controllers/concerns/sunset_concern.rb", FULL_RUNTIME_SUNSET_CONCERN)
        .write("app/controllers/application_controller.rb", RUNTIME_APPLICATION_CONTROLLER)
        .write("app/controllers/widgets_controller.rb", FULL_RUNTIME_WIDGETS_CONTROLLER)
        .write("config/routes.rb", FULL_RUNTIME_ROUTES)
        .write("db/schema.rb", RUNTIME_SCHEMA);
    overlay
        .run_ruby(
            r#"def get(path)
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new("") }
  status, headers, _body = Main.run_rack(env)
  [status, headers["deprecation"], headers["sunset"]]
end

index = get("/widgets")
raise "GET /widgets answered #{index.inspect}" unless index == [200, "@1668384000", "Thu, 01 Oct 2026 00:00:00 GMT"]

show = get("/widgets/1")
raise "GET /widgets/1 answered #{show.inspect}, want no Deprecation/Sunset" unless show == [200, nil, nil]
"#,
        )
        .assert_passes();
}
