//! Dynamic `send` → static `case` dispatch (the ruby-family
//! `apply_send_static_dispatch` pass, run inside `emit_library`).
//!
//! Three source shapes from lobsters, plus the bail case:
//!  A. `as_json` spec-array walk — a local array literal (grown by
//!     `push`) iterated with `send(k)` / `send(k.values.first)`.
//!  B. literal symbol array mapped straight into `send(p)`.
//!  C. `dur.send(intv.downcase)` where the string set flows out of a
//!     hash-literal-returning helper and a frozen const table.
//! Unprovable name sets must leave the `send` untouched.

use roundhouse::App;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

fn emit_ruby(source: &str) -> String {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn spec_array_walk_rewrites_to_case_dispatch() {
    let out = emit_ruby(
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
}

#[test]
fn literal_array_map_rewrites_to_case_dispatch() {
    let out = emit_ruby(
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
    let out = emit_ruby(
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
fn unprovable_name_set_is_left_alone() {
    let out = emit_ruby(
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
}
