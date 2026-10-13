//! Edge shapes of `respond_to` / implicit render negotiation, replayed
//! against actionpack 8.1.3.1 + turbo-rails 2.0.23. `expected.json` is what
//! `tests/support/respond_to_edges/oracle.rb` printed for the SAME controllers
//! (`ruby oracle.rb > expected.json`); shapes the lowering refuses live in
//! `refused_probes_controller.rb`, and the tests for them assert the
//! compile-time error rather than a response.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const PROBES: &str = include_str!("support/respond_to_edges/probes_controller.rb");
const REFUSED: &str = include_str!("support/respond_to_edges/refused_probes_controller.rb");
const EXPECTED: &str = include_str!("support/respond_to_edges/expected.json");
const ACCEPTS: &str = include_str!("support/respond_to_edges/accepts.rb");

const VIEWS: &[&str] = &[
    "probes/any_bare.turbo_stream.erb",
    "probes/h_bare_noview.turbo_stream.erb", "probes/h_cond.html.erb", "probes/h_cond.turbo_stream.erb",
    "probes/h_multi.html.erb", "probes/h_multi.turbo_stream.erb", "probes/h_other.html.erb",
    "probes/h_other_alt.html.erb", "probes/h_redirect.turbo_stream.erb", "probes/h_side.html.erb",
    "probes/h_side.turbo_stream.erb", "probes/h_side_json.html.erb", "probes/h_side_json.turbo_stream.erb",
    "probes/mixed.turbo_stream.erb", "probes/other.html.erb",
    "refused_probes/force_plain.html.erb", "refused_probes/force_plain.turbo_stream.erb",
    "refused_probes/force_turbo.html.erb", "refused_probes/force_turbo.turbo_stream.erb",
    "refused_probes/helper_resp.html.erb", "refused_probes/helper_resp.turbo_stream.erb",
];

fn view(rel: &str) -> String {
    let p = format!("{}/tests/support/respond_to_edges/views/{rel}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(p).expect("view")
}

const ROUTES: &[(&str, &str)] = &[
    ("h_side", "probes"), ("h_side_json", "probes"), ("h_multi", "probes"), ("h_other", "probes"),
    ("h_redirect", "probes"), ("h_bare_noview", "probes"), ("any_bare", "probes"),
    ("mixed", "probes"), ("pin", "probes"), ("semi", "probes"), ("h_cond", "probes"),
];

const REFUSED_ROUTES: &[(&str, &str)] = &[
    ("any_multi", "refused_probes"), ("force_json", "refused_probes"), ("force_turbo", "refused_probes"),
    ("force_plain", "refused_probes"), ("cond_arm", "refused_probes"), ("helper_resp", "refused_probes"),
    ("assign_param", "refused_probes"),
];

fn probes_overlay(file: &str, controller: &str, routes: &[(&str, &str)]) -> emit_and_run::Overlay {
    let mut r = String::from("  root \"articles#index\"\n");
    for (a, c) in routes {
        r.push_str(&format!("  match \"probes/{a}(.:format)\" => \"{c}#{a}\", via: [:get, :post]\n"));
    }
    let mut o = emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", &r)
        .write(&format!("app/controllers/{file}"), controller);
    for v in VIEWS {
        o = o.write(&format!("app/views/{v}"), &view(v));
    }
    o
}

const REPLAY: &str = r##"
require "json"
require "stringio"
expected = JSON.parse(DATA_EXPECTED)
only = ONLY_NAMES
failures = []
expected.each do |row|
  next if only && !only.include?(row["name"])
  path, qs = row["path"].split("?", 2)
  env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => path, "QUERY_STRING" => qs.to_s }.merge(row["env"] || {})
  label = row["accept_label"]
  env["HTTP_ACCEPT"] = ALL.fetch(label) if label
  out = StringIO.new
  got = begin
    Main.run(env, StringIO.new(""), out)
    head, body = out.string.split("\r\n\r\n", 2)
    { "status" => head[/^Status: (\d+)/, 1].to_i,
      "content_type" => head[/^Content-Type: (.*?)\r?$/i, 1],
      "body" => body.to_s }
  rescue Exception => e
    { "status" => -1, "content_type" => nil, "body" => "RAISED #{e.class}: #{e.message[0, 80]}" }
  end
  want = { "status" => row["status"], "content_type" => row["content_type"], "body" => row["body"] }
  norm = ->(h) { h.merge("content_type" => h["content_type"].to_s.downcase.delete(" ")) }
  failures << "DIFF #{row["name"]}\n  rails: #{want.inspect[0, 220]}\n  ours : #{got.inspect[0, 220]}" unless norm.(got) == norm.(want)
end
puts failures.join("\n")
puts "replayed #{expected.length}, #{failures.length} differ"
"##;

/// Replay only the named rows; the test fails when the emitted app answers
/// any of them differently from Rails (or emit reports errors).
fn replay(names: &[&str]) {
    let only = format!("ONLY_NAMES = {:?}\n", names);
    let script = format!(
        "{}\n{}DATA_EXPECTED = <<~'EXPECTED_JSON'\n{}\nEXPECTED_JSON\n{}",
        ACCEPTS, only, EXPECTED, REPLAY
    );
    let run = probes_overlay("probes_controller.rb", PROBES, ROUTES).run_ruby(&script);
    eprintln!("emit errors: {:?}", run.errors);
    assert!(run.success, "ruby failed:\n{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains(", 0 differ"), "differs from Rails:\n{}", run.stdout);
}

/// `format.html { side effects only }` then the implicit render: Rails pins the
/// lookup to the arm's format (html), so a Turbo Accept still gets html.
#[test]
fn html_block_with_side_effects_renders_html_for_a_turbo_accept() {
    replay(&["h_side turbo", "h_side_json turbo", "h_side ws", "h_side upper_turbo", "h_side q0_html"]);
}

/// A bare `format.html` with only a turbo_stream template: Rails answers 406.
#[test]
fn bare_html_arm_with_no_html_template_is_unknown_format() {
    replay(&["h_bare_noview turbo", "h_bare_noview star", "h_bare_noview html_turbo", "h_bare_noview html"]);
}

/// 33+ items: Rails still finds the html item after them.
#[test]
fn accept_item_bound_does_not_change_the_outcome() {
    replay(&["h_multi late_html", "h_multi late_html_q", "h_multi empties_then_html"]);
}

/// `text/*` / `application/*` wildcards and Rails' case-sensitive lookup.
#[test]
fn accept_type_wildcards_and_case() {
    replay(&["h_multi text_star", "h_multi app_star", "h_multi upper"]);
}

/// `?format=json` is `params[:format]`, which beats Accept in Rails.
#[test]
fn query_string_format_pins_the_arm() {
    replay(&["pin ?format=json", "pin ?format=turbo_stream", "h_side_json ?format=json"]);
}

/// The emit errors for the refused shapes (`REFUSED`), each of which the
/// lowering cannot run and so must report rather than emit verbatim.
fn refused_errors() -> Vec<String> {
    let (_emitted, errors) = probes_overlay("refused_probes_controller.rb", REFUSED, REFUSED_ROUTES)
        .emit(BuildTarget::Ruby);
    errors
}

/// `request.format = …` in an action: Rails re-pins the request's formats,
/// the runtime's request has no setter. An error, not a NoMethodError.
#[test]
fn request_format_assignment_is_an_error() {
    let errors = refused_errors();
    let n = errors.iter().filter(|e| e.contains("request.format = ")).count();
    assert_eq!(n, 3, "force_json, force_turbo and force_plain: {errors:?}");
}

/// A conditional arm registration (`format.json { } if cond`) would be
/// emitted verbatim with `format` unbound.
#[test]
fn conditional_arm_registration_is_an_error() {
    let errors = refused_errors();
    assert!(errors.iter().any(|e| e.contains("registered conditionally")), "{errors:?}");
}

/// `format.any(:json, :turbo_stream) { }` would answer formats it never named.
#[test]
fn format_any_with_a_mime_list_is_an_error() {
    let errors = refused_errors();
    assert!(errors.iter().any(|e| e.contains("format.any") && e.contains("mime list")), "{errors:?}");
}

/// A bare `format.turbo_stream` in a private helper has no action whose
/// template it could render.
#[test]
fn respond_to_with_bare_arms_in_a_private_helper_is_an_error() {
    let errors = refused_errors();
    assert!(
        errors.iter().any(|e| e.contains("format.turbo_stream") && e.contains("not in a public action body")),
        "{errors:?}"
    );
}

/// Renaming the block parameter to `format` is unsound when the block
/// writes to the parameter: the write would land on the action's own
/// `format` local. (Reading such a local is fine: `shadow_local`.)
#[test]
fn a_write_to_a_renamed_block_parameter_is_an_error() {
    let errors = refused_errors();
    assert!(errors.iter().any(|e| e.contains("assigns to its own parameter")), "{errors:?}");
}

/// Sanity: shapes that SHOULD already agree (control group).
#[test]
fn control_group_agrees() {
    replay(&["h_other turbo", "h_other json", "h_multi turbo", "h_multi json", "h_cond turbo", "mixed turbo",
             "mixed html", "any_bare turbo", "h_side html", "h_side json", "h_multi ws", "h_multi badq",
             "h_multi dupq", "h_multi spq", "h_multi negq", "h_multi expq", "h_multi long_item", "h_multi params"]);
}
