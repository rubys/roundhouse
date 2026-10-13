//! `as_json` on a typed Hash or Array is ActiveSupport's core_ext reopen,
//! which no transpiled runtime hosts: on Spinel the request's
//! `params.as_json` (an action logging what it received) and a literal
//! Hash's raised "undefined method 'as_json' for an instance of Hash".
//! It is grounded as `ActiveSupport::JSON.jsonable(...)`, the conversion
//! the runtime already ships: String keys, Symbols as Strings, nested
//! containers converted all the way down.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const INDEX: &str = "    @articles = Article.includes(:comments).order(created_at: :desc)\n";

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            INDEX,
            &format!(
                "    if params[:probe] == \"1\"\n      seen = params.as_json\n      echo = {{ kind: :widget, tags: [:a, \"b\"], nested: {{ size: 2 }}, at: Time.utc(2026, 10, 12, 10, 0, 0) }}.as_json\n      render plain: \"#{{seen[\"note\"]}}|#{{echo[\"kind\"]}}|#{{echo[\"tags\"].join(\",\")}}|#{{echo[\"nested\"][\"size\"]}}|#{{echo[\"at\"]}}\"\n      return\n    end\n{INDEX}"
            ),
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "  test \"should get new\" do",
            "  test \"as_json on params and a literal hash\" do\n    get articles_url, params: { probe: \"1\", note: \"gadget\" }\n    assert_equal \"gadget|widget|a,b|2|2026-10-12T10:00:00.000Z\", response.body\n  end\n\n  test \"should get new\" do",
        )
}

#[test]
fn hash_as_json_runs_and_is_grounded_for_spinel() {
    overlay().run_test("test/controllers/articles_controller_test.rb").assert_passes();

    let (emitted, _errors) = overlay().emit(BuildTarget::Spinel);
    let src = std::fs::read_to_string(emitted.join("app/controllers/articles_controller.rb"))
        .expect("emitted articles controller");
    assert!(!src.contains(".as_json"), "a typed as_json survived:\n{src}");
    assert_eq!(src.matches("ActiveSupport::JSON.jsonable(").count(), 2, "{src}");
}

/// A Time inside the converted Hash is written as Rails writes it:
/// `xmlschema(3)`, three fractional digits, `Z` for UTC and the offset
/// otherwise. The runtime's Times stand in for `TimeWithZone` values in
/// the app's zone, so the expected strings come from Rails' own
/// `TimeWithZone#as_json`.
#[test]
fn jsonable_writes_a_time_as_rails_does() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require "active_support"
require "active_support/time"
require "active_support/core_ext/object/json"

# Rails' answers first: the runtime file below reopens ActiveSupport
# classes (TimeZone among them) with its own shapes.
utc = Time.utc(2026, 10, 12, 10, 0, 0, 123456)
whole = Time.utc(2026, 10, 12, 10, 0, 0)
want = {
  "UTC" => utc.in_time_zone("UTC").as_json,
  "Madrid" => utc.in_time_zone("Madrid").as_json,
  "Lima" => utc.in_time_zone("Lima").as_json,
  "nested" => { at: whole.in_time_zone("UTC"), list: [whole.in_time_zone("UTC")] }.as_json,
}
madrid = Time.new(2026, 10, 12, 12, 0, 0.123456r, "+02:00")
lima = Time.new(2026, 10, 12, 5, 0, 0.123456r, "-05:00")

load ARGV.fetch(0)

got = {
  "UTC" => ActiveSupport::JSON.jsonable(utc),
  "Madrid" => ActiveSupport::JSON.jsonable(madrid),
  "Lima" => ActiveSupport::JSON.jsonable(lima),
  "nested" => ActiveSupport::JSON.jsonable({ at: whole, list: [whole] }),
}
want.each do |what, rails|
  raise "#{what}: got #{got[what].inspect}, Rails #{rails.inspect}" unless got[what] == rails
end
puts "time contracts passed"
"##;
    let output = std::process::Command::new("ruby")
        .arg("-e")
        .arg(script)
        .arg(root.join("runtime/ruby/active_support_ext.rb"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("time contracts passed"),
        "=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
