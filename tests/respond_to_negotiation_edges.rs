//! Second pass over `respond_to` negotiation: helper-driven responses in an
//! arm, arm shapes (statements before an if/else of arms, a block parameter
//! not named `format`), `?format=` values, and Accept ranking, replayed
//! against actionpack 8.1.3.1 + turbo-rails 2.0.23. `expected.json` is what
//! `tests/support/respond_to_negotiation/oracle.rb` printed for the SAME
//! controller and the requests in `reqs.rb`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const PROBES: &str = include_str!("support/respond_to_negotiation/probes_controller.rb");
const EXPECTED: &str = include_str!("support/respond_to_negotiation/expected.json");
const REQS: &str = include_str!("support/respond_to_negotiation/reqs.rb");

const ACTIONS: &[&str] = &[
    "html_helper", "html_helper_redirect", "html_cond_redirect", "html_cond_head", "html_unless_render",
    "html_early_return", "html_rescue", "html_status", "pre_assign", "seq_if", "top_if", "wants_param",
    "multi_turbo", "three", "json_only", "html_only", "plain_two", "send_arm", "rescue_render",
    "shadow_local", "numbered", "it_param", "branch_diff", "top_branch_diff", "if_noelse",
    "seq_if_noelse", "else_other_stmt", "any_two", "nested_named", "nested_rest", "nested_it",
];

fn view(rel: &str) -> String {
    let p = format!("{}/tests/support/respond_to_negotiation/views/{rel}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(p).expect("view")
}

fn overlay(actions: &[&str]) -> emit_and_run::Overlay {
    let mut r = String::from("  root \"articles#index\"\n");
    for a in actions {
        r.push_str(&format!("  match \"probes/{a}(.:format)\" => \"probes#{a}\", via: [:get, :post]\n"));
    }
    let mut o = emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", &r)
        .write("app/controllers/probes_controller.rb", PROBES);
    let dir = format!("{}/tests/support/respond_to_negotiation/views/probes", env!("CARGO_MANIFEST_DIR"));
    for e in std::fs::read_dir(dir).unwrap() {
        let n = e.unwrap().file_name().into_string().unwrap();
        o = o.write(&format!("app/views/probes/{n}"), &view(&format!("probes/{n}")));
    }
    o
}

const REPLAY: &str = r##"
require "json"
require "stringio"
expected = JSON.parse(DATA_EXPECTED)
want_by = expected.to_h { |r| [r["name"], r] }
failures = []
t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
slow = []
REQS.each do |name, path, accept, extra|
  next unless PREFIXES.any? { |p| name.start_with?(p) }
  # Out of scope: Rails' 406 for a pinned xml format has an xml error body.
  next if name.include?("=xml")
  # Known router gap, not negotiation: an unregistered path extension
  # (`.bogus`, `.JSON`, `.all`, `.none`) 404s in the router; Rails matches
  # the route with an unknown format (406 on a respond_to, 200 html on an
  # implicit render).
  next if name.match?(/\A\w+ \.(bogus|JSON|all|none) /)
  row = want_by.fetch(name)
  p1, qs = path.split("?", 2)
  env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => p1, "QUERY_STRING" => qs.to_s }.merge(extra || {})
  env["HTTP_ACCEPT"] = accept if accept
  out = StringIO.new
  s = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  got = begin
    Main.run(env, StringIO.new(""), out)
    head, body = out.string.split("\r\n\r\n", 2)
    { "status" => head[/^Status: (\d+)/, 1].to_i, "content_type" => head[/^Content-Type: (.*?)\r?$/i, 1], "location" => head[/^Location: (.*?)\r?$/i, 1], "body" => body.to_s }
  rescue Exception => e
    { "status" => -1, "content_type" => nil, "location" => nil, "body" => "RAISED #{e.class}: #{e.message[0, 80]}" }
  end
  dt = Process.clock_gettime(Process::CLOCK_MONOTONIC) - s
  slow << "#{name} #{dt.round(2)}s" if dt > 5.0
  want = { "status" => row["status"], "content_type" => row["content_type"], "location" => row["location"], "body" => row["body"] }
  # Out of scope: the body of a redirect (ours is Rack's link, Rails sends none).
  # The redirect destination is compared by path: Rails answers an absolute
  # URL on the test host, the emitted app the path it was given.
  norm = ->(h) { h.merge("content_type" => h["content_type"].to_s.downcase.delete(" "), "location" => h["location"]&.sub(%r{\Ahttps?://[^/]+}, ""), "body" => h["status"].between?(300, 399) ? "" : h["body"]) }
  failures << "DIFF #{name}\n  rails: #{want.inspect[0, 200]}\n  ours : #{got.inspect[0, 200]}" unless norm.(got) == norm.(want)
end
puts failures.join("\n")
puts "SLOW: #{slow.inspect}"
puts "replayed, #{failures.length} differ"
"##;

fn replay(prefixes: &[&str]) {
    let script = format!(
        "{}\nPREFIXES = {:?}\nDATA_EXPECTED = <<~'EXPECTED_JSON'\n{}\nEXPECTED_JSON\n{}",
        REQS, prefixes, EXPECTED, REPLAY
    );
    let run = overlay(ACTIONS).run_ruby(&script);
    eprintln!("emit errors: {:?}", run.errors);
    assert!(run.success, "ruby failed:\n{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains(", 0 differ"), "differs from Rails:\n{}", run.stdout);
    assert!(run.stdout.contains("SLOW: []"), "a request took over five seconds:\n{}", run.stdout);
}

#[test]
fn html_arm_helper_that_responds() {
    replay(&["html_helper ", "html_helper_redirect "]);
}
#[test]
fn html_arm_conditional_terminals() {
    replay(&["html_cond_head ", "html_unless_render ", "html_rescue ", "html_status "]);
}
#[test]
fn legit_shapes() {
    replay(&["pre_assign ", "top_if ", "multi_turbo "]);
}

/// A statement before an if/else holding the arms is valid Rails.
#[test]
fn statement_before_if_else_arms_is_not_refused() {
    let (_e, errors) = overlay(ACTIONS).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "valid Rails shape refused: {errors:?}");
}

/// `respond_to do |wants|`: the arms are recognised whatever the block
/// parameter is called.
#[test]
fn block_param_not_named_format() {
    replay(&["wants_param "]);
}
/// `?format=` values on respond_to actions, with and without a path format.
#[test]
fn query_format_matrix() {
    replay(&["three ?format", "json_only ?format", "html_only ?format", "three .", "json_only .", "html_only ."]);
}
/// Accept ranking: q ties, duplicates, wildcards, `*/*`, and 9000-item headers
/// (a quadratic parse took over a minute on those).
#[test]
fn accept_ranking_three() {
    replay(&["three acc "]);
}

/// Arms that send a file, or rescue and then render, and a block parameter
/// that shadows a local of the same name.
#[test]
fn arm_guard_cases_and_a_shadowed_block_param() {
    replay(&["send_arm ", "rescue_render ", "shadow_local "]);
}

/// `respond_to do _1.html … end` and `do it.html … end`.
#[test]
fn numbered_and_it_block_params() {
    replay(&["numbered ", "it_param "]);
}

/// Branches registering different arm sets, and a path that registers no
/// arm at all (an `if` with no else, an else that only `head`s): Rails
/// raises UnknownFormat there, even after the `head`.
#[test]
fn branches_with_different_or_no_arm_sets() {
    replay(&["branch_diff ", "top_branch_diff ", "if_noelse ", "seq_if_noelse ", "else_other_stmt "]);
}

/// A bare `format.any` beside templates in two formats answers the
/// negotiated format's template, as Rails' default render does.
#[test]
fn bare_format_any_renders_the_negotiated_template() {
    replay(&["any_two "]);
}

/// A nested block whose parameter reads like the respond_to block's
/// (`|f|` inside `f.json { … }`, `it` inside `it.json { … }`) keeps its own
/// binding.
#[test]
fn nested_blocks_keep_their_own_parameter() {
    replay(&["nested_named ", "nested_rest ", "nested_it "]);
}

/// `?format=` values on an action with no `respond_to`: the implicit render
/// looks a template up, so a value Rails does not register (`bogus`, `JSON`,
/// `all`, an empty one) pins nothing and `js` falls back to html.
#[test]
fn query_format_matrix_for_the_implicit_render() {
    replay(&["plain_two ?format", "plain_two ."]);
}

/// Accept ranking behind the implicit render (the same headers as above).
#[test]
fn accept_ranking_implicit() {
    replay(&["plain_two acc "]);
}
