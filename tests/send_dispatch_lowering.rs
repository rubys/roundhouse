//! Dynamic `send` → static `case` dispatch (the shared
//! `lower::apply_send_static_dispatch` pass, run on the post-analyze
//! hook).
//!
//! Three source shapes from lobsters, plus the bail case:
//!  A. `as_json` spec-array walk — a local array literal (grown by
//!     `push`) iterated with `send(k)` / `send(k.values.first)`.
//!  B. literal symbol array mapped straight into `send(p)`.
//!  C. `dur.send(intv.downcase)` where the string set flows out of a
//!     hash-literal-returning helper and a frozen const table — in
//!     both call forms: qualified (`IntervalHelper.time_interval`) and
//!     the bare mixed-in call lobsters actually writes (the emit-time
//!     ancestor of this pass only saw the qualified form, post
//!     helper-lowering).
//! Unprovable name sets must leave the `send` untouched and go on the
//! residue ledger.

use roundhouse::App;
use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::Diagnostic;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::{apply_duration_lowering, apply_send_static_dispatch};

/// Ingest → analyze → shared send grounding → duration grounding →
/// ruby render, mirroring the post-analyze hook's pass order (duration
/// AFTER dispatch — the all-duration-unit arms below count on it).
/// Returns the emitted source plus the dispatch pass's residue ledger.
fn ground_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags = apply_send_static_dispatch(&mut app, analyzer.class_registry());
    apply_duration_lowering(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn spec_array_walk_rewrites_to_case_dispatch() {
    let (out, diags) = ground_and_emit(
        r##"
class Story
  def as_json(options = {})
    h = [:short_id, :title, { submitter_user: :user }]
    h.push(comments: options[:with_comments]) if options
    js = {}
    h.each do |k|
      if k.is_a?(Symbol)
        js[k] = send(k)
      elsif k.is_a?(Hash) && k.values.first.is_a?(Symbol)
        js[k.keys.first] = send(k.values.first)
      end
    end
    js
  end
end
"##,
    );
    // Direct dispatch: the two symbol elements, first-seen order.
    assert!(out.contains("when :short_id"), "direct arm missing:\n{out}");
    assert!(out.contains("when :title"), "direct arm missing:\n{out}");
    // Projection dispatch: the hash element's first value. The pushed
    // hash's value (`options[:with_comments]`) is not a symbol literal
    // and contributes nothing.
    assert!(out.contains("when :user"), "projection arm missing:\n{out}");
    // The unknown-name fallback keeps send's loud-failure behavior and
    // renders as `else`, never the invalid `when _`.
    assert!(out.contains("raise \"dynamic send"), "fallback arm missing:\n{out}");
    assert!(!out.contains("when _"), "wildcard arm rendered as `when _`:\n{out}");
    assert!(!out.contains("send(k"), "dynamic send survived:\n{out}");
    assert!(diags.is_empty(), "grounded sites must not ledger residue: {diags:?}");
}

#[test]
fn literal_array_map_rewrites_to_case_dispatch() {
    let (out, _) = ground_and_emit(
        r##"
class Search
  attr_reader :q, :what, :order
  def to_url_params
    [:q, :what, :order].map { |p| "#{p}=#{send(p).to_s}" }.join("&")
  end
end
"##,
    );
    for arm in ["when :q", "when :what", "when :order"] {
        assert!(out.contains(arm), "{arm} missing:\n{out}");
    }
    assert!(!out.contains("send(p"), "dynamic send survived:\n{out}");
}

#[test]
fn helper_hash_string_set_rewrites_to_case_dispatch() {
    let (out, _) = ground_and_emit(
        r##"
module IntervalHelper
  TIME_INTERVALS = { "h" => "Hour", "d" => "Day" }.freeze

  def self.time_interval(param)
    if param == "1d"
      { dur: 1, intv: TIME_INTERVALS[param] }
    else
      { dur: 2, intv: "Week" }
    end
  end
end

class FlaggedCommenters
  def initialize(interval)
    length = IntervalHelper.time_interval(interval)
    @period = length[:dur].send(length[:intv].downcase).ago
  end
end
"##,
    );
    // Const-table values + the else-literal, lowercased; all duration
    // units, so the arms call the plural form (which the downstream
    // duration pass grounds to the Duration runtime).
    for arm in ["when \"hour\"", "when \"day\"", "when \"week\""] {
        assert!(out.contains(arm), "{arm} missing:\n{out}");
    }
    assert!(
        out.contains("ActiveSupport::Duration.weeks(length[:dur])"),
        "duration grounding missing:\n{out}"
    );
    assert!(!out.contains(".send("), "dynamic send survived:\n{out}");
}

#[test]
fn bare_mixed_in_helper_call_matches_provider() {
    // The lobsters source shape: `include IntervalHelper` + a BARE
    // `time_interval(...)` call. At hook time helper-qualification
    // hasn't happened, so the provider scan attributes the bare call by
    // app-wide-unique method name.
    let (out, diags) = ground_and_emit(
        r##"
module IntervalHelper
  TIME_INTERVALS = { "h" => "Hour", "d" => "Day" }.freeze

  def time_interval(param)
    if param == "1d"
      { dur: 1, intv: TIME_INTERVALS[param] }
    else
      { dur: 2, intv: "Week" }
    end
  end
end

class FlaggedCommenters
  include IntervalHelper

  def initialize(interval)
    length = time_interval(interval)
    @period = length[:dur].send(length[:intv].downcase).ago
  end
end
"##,
    );
    for arm in ["when \"hour\"", "when \"day\"", "when \"week\""] {
        assert!(out.contains(arm), "{arm} missing:\n{out}");
    }
    assert!(!out.contains(".send("), "dynamic send survived:\n{out}");
    assert!(diags.is_empty(), "grounded sites must not ledger residue: {diags:?}");
}

#[test]
fn unprovable_name_set_is_left_alone() {
    let (out, diags) = ground_and_emit(
        r##"
class Foo
  def dyn(name)
    send(name)
  end
end
"##,
    );
    assert!(
        out.contains("send(name)"),
        "send with an unprovable name set must survive untouched:\n{out}"
    );
    // The site joins the residue ledger — strict targets report it as
    // an honest per-target gap instead of failing silently at compile.
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
    assert!(
        diags[0].to_string().contains("not statically enumerable"),
        "unexpected residue reason: {}",
        diags[0]
    );
}

#[test]
fn provider_returning_a_constant_and_a_multi_assigned_local() {
    // Current lobsters' `time_interval`: bad input returns the
    // PLACEHOLDER constant, and `intv:` is a local bound by a multiple
    // assignment from an `if` of array literals.
    let (out, diags) = ground_and_emit(
        r##"
module IntervalHelper
  PLACEHOLDER = {param: "1w", dur: 7, intv: "Day", placeholder: true}
  TIME_INTERVALS = {"h" => "Hour", "w" => "Week", "m" => "Month"}.freeze

  def self.time_interval(param)
    if (m = param.to_s.match(/\A(\d+)([hwm])\z/))
      dur = m[1].to_i
      return PLACEHOLDER unless dur > 0
      intv = TIME_INTERVALS[m[2]]
      sqlite_dur, sqlite_intv =
        if intv == "Week"
          [dur * 7, "Day"]
        else
          [dur, intv]
        end
      {param: param, dur: sqlite_dur, intv: sqlite_intv}
    else
      PLACEHOLDER
    end
  end
end

class FlaggedCommenters
  def initialize(interval)
    length = IntervalHelper.time_interval(interval)
    @period = length[:dur].send(length[:intv].downcase).ago
  end
end
"##,
    );
    for arm in ["when \"day\"", "when \"hour\"", "when \"week\"", "when \"month\""] {
        assert!(out.contains(arm), "{arm} missing:\n{out}");
    }
    assert!(!out.contains(".send("), "dynamic send survived:\n{out}");
    assert!(diags.is_empty(), "grounded sites must not ledger residue: {diags:?}");
}

#[test]
fn a_helper_spliced_into_several_includers_is_still_one_definition() {
    // `include IntervalHelper` in a controller splices a copy of
    // `time_interval` into it; the copies share the one `def`'s span and
    // must not make the bare call in the model look ambiguous.
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "app/helpers/interval_helper.rb",
            "module IntervalHelper\n  TIME_INTERVALS = {\"h\" => \"Hour\", \"d\" => \"Day\"}.freeze\n\n  def time_interval(param)\n    {dur: 1, intv: TIME_INTERVALS[param]}\n  end\nend\n",
        ),
        (
            "app/controllers/home_controller.rb",
            "class HomeController < ActionController::Base\n  include IntervalHelper\n\n  def index\n    @i = time_interval(params[:t])\n  end\nend\n",
        ),
        (
            "app/models/flagged_commenters.rb",
            "class FlaggedCommenters\n  include IntervalHelper\n\n  def initialize(interval)\n    length = time_interval(interval)\n    @period = length[:dur].send(length[:intv].downcase).ago\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/\" => \"home#index\"\nend\n"),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_library(&app).into_iter().map(|f| f.content).collect::<Vec<_>>().join("\n");
    assert!(out.contains("when \"hour\""), "got:\n{out}");
    assert!(!out.contains(".send("), "dynamic send survived:\n{out}");
}
