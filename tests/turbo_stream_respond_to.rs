//! `respond_to` with a `format.turbo_stream` arm: the emitted program has
//! to answer what Rails answers.
//!
//! A Turbo form submission negotiates `Accept: text/vnd.turbo-stream.html,
//! text/html, application/xhtml+xml`, and Rails runs the `turbo_stream` arm
//! for it. Before this lowering the arm was dropped with a WARNING, so every
//! such action (comments, votes, follows, bookmarks) answered its html
//! branch to a Turbo request with the strict gate green.
//!
//! The expected table is MEASURED, not derived: `tests/support/
//! turbo_respond_to/oracle.rb` runs the same probe controller under
//! actionpack 8.1.3.1 + turbo-rails 2.0.23 against ten Accept headers and
//! two pinned formats, and `expected.json` is what it printed. Each case
//! asserts status, Content-Type and body. They cover:
//!
//! - arm selection by Accept, including the q-value order, a browser's
//!   `*/*,` list reading as html, and `text/html, text/vnd.turbo-stream.html`
//!   (html FIRST) selecting html although a turbo_stream arm exists;
//! - `*/*` taking the FIRST registered arm (`turbo_first`);
//! - a request no arm answers raising `UnknownFormat`: 406, html page for
//!   html, the json status document for a json request;
//! - `format.any` catching what no arm above claims;
//! - the bare arm (implicit `<action>.turbo_stream.erb`), the `render :sym`
//!   arm, the `head` arm, a block that only has side effects and then falls
//!   to the implicit template, and inline builders
//!   (`render turbo_stream: turbo_stream.update(…, partial:, locals:)`,
//!   singly and as an Array, with `status:`);
//! - a bare partial name found in an ANCESTOR's view directory
//!   (`partial: "banner"` in `application/`), an interpolated target, and
//!   a path-pinned format;
//! - the json arms (`render json: { … }`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const PROBES: &str = include_str!("support/turbo_respond_to/probes_controller.rb");
const EXPECTED: &str = include_str!("support/turbo_respond_to/expected.json");
const FLASH_PARTIAL: &str = include_str!("support/turbo_respond_to/views/probes/_flash.html.erb");
const IMPLICIT_TEMPLATE: &str =
    include_str!("support/turbo_respond_to/views/probes/implicit.turbo_stream.erb");
const HTML_BARE_HTML: &str = include_str!("support/turbo_respond_to/views/probes/html_bare.html.erb");
const HTML_BARE_TURBO: &str =
    include_str!("support/turbo_respond_to/views/probes/html_bare.turbo_stream.erb");
const BANNER_PARTIAL: &str = include_str!("support/turbo_respond_to/views/application/_banner.html.erb");
const AFTER_BLOCK_TEMPLATE: &str =
    include_str!("support/turbo_respond_to/views/probes/after_block.turbo_stream.erb");

const PROBE_ACTIONS: &[&str] = &[
    "implicit", "inline", "many", "head_arm", "render_sym", "after_block", "turbo_only",
    "turbo_first", "html_json", "any_arm", "inherited",
    "html_bare",
];

fn routes_for(actions: &[&str]) -> String {
    let mut out = String::from("  root \"articles#index\"\n");
    for a in actions {
        out.push_str(&format!("  match \"probes/{a}\" => \"probes#{a}\", via: [:get, :post]\n"));
    }
    out
}

fn probes() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", &routes_for(PROBE_ACTIONS))
        .write("app/controllers/probes_controller.rb", PROBES)
        .write("app/views/probes/_flash.html.erb", FLASH_PARTIAL)
        .write("app/views/application/_banner.html.erb", BANNER_PARTIAL)
        .write("app/views/probes/implicit.turbo_stream.erb", IMPLICIT_TEMPLATE)
        .write("app/views/probes/html_bare.html.erb", HTML_BARE_HTML)
        .write("app/views/probes/html_bare.turbo_stream.erb", HTML_BARE_TURBO)
        .write("app/views/probes/after_block.turbo_stream.erb", AFTER_BLOCK_TEMPLATE)
}

/// Every measured request, replayed through the emitted app's own
/// dispatcher (`Main.run`, the CGI-shaped entry the Rack and spinel
/// servers share) and compared field by field.
const REPLAY: &str = r##"
require "json"
require "stringio"
expected = JSON.parse(DATA_EXPECTED)
failures = []
expected.each do |row|
  env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => row["path"] }
  env["HTTP_ACCEPT"] = row["accept"] unless row["accept"].nil?
  out = StringIO.new
  Main.run(env, StringIO.new(""), out)
  head, body = out.string.split("\r\n\r\n", 2)
  got = {
    "status" => head[/^Status: (\d+)/, 1].to_i,
    "content_type" => head[/^Content-Type: (.*?)\r?$/i, 1],
    "body" => body.to_s,
  }
  want = { "status" => row["status"], "content_type" => row["content_type"], "body" => row["body"] }
  # Header case and spacing are not behavior: `charset=UTF-8` vs `utf-8`.
  norm = ->(h) { h.merge("content_type" => h["content_type"].to_s.downcase.delete(" ")) }
  failures << "#{row["name"]} (Accept: #{row["accept"].inspect})\n  rails: #{want.inspect}\n  ours : #{got.inspect}" unless norm.(got) == norm.(want)
end
raise "#{failures.length} of #{expected.length} responses differ from Rails:\n" + failures.join("\n") unless failures.empty?
puts "replayed #{expected.length} responses"
"##;

fn replay_script() -> String {
    let literal = format!("DATA_EXPECTED = <<~'EXPECTED_JSON'\n{EXPECTED}\nEXPECTED_JSON\n");
    format!("{literal}{REPLAY}")
}

#[test]
fn every_probe_answers_what_rails_answers_for_every_accept() {
    let run = probes().run_ruby(&replay_script());
    run.assert_passes();
    assert!(run.stdout.contains("replayed 124 responses"), "stdout: {}", run.stdout);
}

#[test]
fn the_spinel_tree_lowers_the_same_controller_without_errors() {
    // The replay above runs the CRuby tree. The spinel tree lowers the same
    // controller with the narrower breadth (no CRuby-only JsonRender), and a
    // refusal there would be an ERROR; emitting it clean is the claim that
    // every probe arm is carried by the AOT tree too.
    let (_emitted, errors) = probes().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "spinel tree reports errors:\n{}", errors.join("\n"));
}

#[test]
fn the_dispatch_is_rails_negotiation_not_a_path_sniff() {
    let (emitted, errors) = probes().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let src = std::fs::read_to_string(emitted.join("app/controllers/probes_controller.rb"))
        .expect("emitted probes controller");
    // Arms in source order, tested through the runtime's negotiation…
    assert!(
        src.contains(r#"negotiate_format(["turbo_stream", "html"]) == "turbo_stream""#)
            && src.contains(r#"negotiate_format(["html", "turbo_stream", "json"]) == "json""#),
        "{src}"
    );
    // …and what no arm answers is UnknownFormat, never the html arm.
    assert!(src.contains("raise ActionController::UnknownFormat.new"), "{src}");
}

fn refused(controller: &str, view: Option<(&str, &str)>) -> Vec<String> {
    let mut overlay = emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", &routes_for(&["probe"]))
        .write("app/controllers/probes_controller.rb", controller);
    if let Some((path, content)) = view {
        overlay = overlay.write(path, content);
    }
    let (_emitted, errors) = overlay.emit(BuildTarget::Ruby);
    errors
}

#[test]
fn a_bare_turbo_arm_with_no_template_is_an_error_not_a_dropped_arm() {
    // Rails answers 204 to a POST here and raises MissingExactTemplate to a
    // browser GET, a split the lowering does not model. Refused, not guessed.
    let errors = refused(
        "class ProbesController < ApplicationController\n  def probe\n    respond_to do |format|\n      format.html { render html: \"h\" }\n      format.turbo_stream\n    end\n  end\nend\n",
        None,
    );
    assert!(
        errors.iter().any(|e| e.contains("format.turbo_stream") && e.contains("no template")),
        "{errors:?}"
    );
}

#[test]
fn markup_built_by_app_code_is_an_error() {
    let errors = refused(
        "class ProbesController < ApplicationController\n  def probe\n    respond_to do |format|\n      format.html { render html: \"h\" }\n      format.turbo_stream { render turbo_stream: build_streams(view_context) }\n    end\n  end\n\n  private\n\n  def build_streams(_view)\n    \"\"\n  end\nend\n",
        None,
    );
    assert!(
        errors.iter().any(|e| e.contains("format.turbo_stream") && e.contains("builder call")),
        "{errors:?}"
    );
}

#[test]
fn a_builder_option_the_markup_writer_lacks_is_an_error() {
    // `method: :morph` adds an attribute to the element; the runtime's
    // fragment composer would silently omit it.
    let errors = refused(
        "class ProbesController < ApplicationController\n  def probe\n    respond_to do |format|\n      format.turbo_stream { render turbo_stream: turbo_stream.replace(\"x\", partial: \"probes/flash\", method: :morph) }\n    end\n  end\nend\n",
        Some(("app/views/probes/_flash.html.erb", FLASH_PARTIAL)),
    );
    assert!(errors.iter().any(|e| e.contains("only `partial:` and `locals:`")), "{errors:?}");
}

#[test]
fn an_unmodeled_format_arm_is_an_error() {
    let errors = refused(
        "class ProbesController < ApplicationController\n  def probe\n    respond_to do |format|\n      format.html { render html: \"h\" }\n      format.pdf { head :ok }\n    end\n  end\nend\n",
        None,
    );
    assert!(errors.iter().any(|e| e.contains("format.pdf")), "{errors:?}");
}

#[test]
fn accept_parsing_is_exact_and_linear() {
    let script = r##"
# Exact: a late q=1 turbo item outranks 32 earlier html q=0.5 items, as in
# Rails; unknown types and empty items change nothing.
many = (["text/html;q=0.5"] * 32 + ["text/vnd.turbo-stream.html"] * 8).join(",")
raise ActionController.accepted_formats(many).inspect unless ActionController.accepted_formats(many) == ["turbo_stream", "html"]
late = (["image/png"] * 33 + ["text/html"]).join(",")
raise "late html" unless ActionController.accepted_formats(late) == ["png", "html"]
raise "empties" unless ActionController.accepted_formats((["" ] * 40 + ["text/html"]).join(",")) == ["html"]
# The best item of a format counts, wherever it sits; ties keep header order.
raise "best q" unless ActionController.accepted_formats("text/html;q=0.1, application/json;q=0.5, text/html;q=0.9") == ["html", "json"]
raise "tie" unless ActionController.accepted_formats("application/json, text/html, application/json") == ["json", "html"]
# A header can break across lines: `text/html,\n*/*` is a browser list.
raise "newline" unless ActionController.accepted_formats("text/html,\n*/*") == ["html"]
# Linear in the header, not quadratic: 210000 items answer at once.
t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
huge = (["text/html;q=0.5", "application/json;q=0.4", "x/y"] * 70_000).join(",")
raise "huge" unless ActionController.accepted_formats(huge) == ["html", "json"]
raise "slow" if Process.clock_gettime(Process::CLOCK_MONOTONIC) - t > 5
puts "accept ok"
"##;
    let run = probes().run_ruby(script);
    run.assert_passes();
    assert!(run.stdout.contains("accept ok"), "stdout: {}", run.stdout);
}
