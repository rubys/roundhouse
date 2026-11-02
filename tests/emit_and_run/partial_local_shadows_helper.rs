//! A partial local named like a zero-argument view helper
//! (`csrf_meta_tags`, `csrf_meta_tag`, `csp_meta_tag`,
//! `javascript_importmap_tags`) is a local in Rails: Ruby reads the bare
//! name as the lvar the partial was handed. The view lowering rewrote
//! it to the helper anyway, so `<%= csrf_meta_tags %>` rendered the
//! helper's output (nothing, with forgery protection off) and the
//! caller's value was lost. Only a local shadows the helper: an
//! `@csrf_meta_tags` the view reads, which becomes a param of that
//! name, does not, and `csrf_meta_tags()` calls the helper past one.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  layout false\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/strict\", to: \"posts#strict\"\n  get \"/plain\", to: \"posts#plain\"\n  get \"/ivar\", to: \"posts#ivar\"\nend\n",
        )
        .write(
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def strict\n  end\n\n  def plain\n  end\n\n  def ivar\n    @csrf_meta_tags = \"I1\"\n  end\nend\n",
        )
        .write(
            "app/views/posts/strict.html.erb",
            "<%= render \"posts/banner\", csrf_meta_tags: \"S1\", csrf_meta_tag: \"S2\", csp_meta_tag: \"S3\", javascript_importmap_tags: \"S4\" %>\n",
        )
        .write(
            "app/views/posts/_banner.html.erb",
            "<%# locals: (csrf_meta_tags:, csrf_meta_tag:, csp_meta_tag:, javascript_importmap_tags:) %>\n\
             <p id=\"read\"><%= csrf_meta_tags %>|<%= csrf_meta_tag %>|<%= csp_meta_tag %>|<%= javascript_importmap_tags %></p>\n\
             <p id=\"or\"><%= csrf_meta_tags || \"dflt\" %>|<%= csrf_meta_tag || \"dflt\" %>|<%= csp_meta_tag || \"dflt\" %>|<%= javascript_importmap_tags || \"dflt\" %></p>\n\
             <p id=\"call\"><%= csrf_meta_tags() %></p>\n",
        )
        .write(
            "app/views/posts/plain.html.erb",
            "<%= render partial: \"posts/plain_banner\", locals: { csrf_meta_tags: \"P1\", csrf_meta_tag: \"P2\", csp_meta_tag: \"P3\", javascript_importmap_tags: \"P4\" } %>\n",
        )
        .write(
            "app/views/posts/_plain_banner.html.erb",
            "<p id=\"read\"><%= csrf_meta_tags %>|<%= csrf_meta_tag %>|<%= csp_meta_tag %>|<%= javascript_importmap_tags %></p>\n\
             <p id=\"or\"><%= csrf_meta_tags || \"dflt\" %>|<%= csrf_meta_tag || \"dflt\" %>|<%= csp_meta_tag || \"dflt\" %>|<%= javascript_importmap_tags || \"dflt\" %></p>\n\
             <p id=\"call\"><%= csrf_meta_tags() %></p>\n",
        )
        .write(
            "app/views/posts/ivar.html.erb",
            "<p id=\"view\"><%= @csrf_meta_tags %>|<%= csrf_meta_tags %></p>\n<%= render \"posts/ivar_banner\" %>\n",
        )
        .write(
            "app/views/posts/_ivar_banner.html.erb",
            "<p id=\"partial\"><%= @csrf_meta_tags %>|<%= csrf_meta_tags %></p>\n",
        )
}

/// The helper's first tag, quoted for a Ruby string literal.
const META: &str = r#"<meta name=\"csrf-param\" content=\"authenticity_token\" />"#;

const GET: &str = r#"
def get(path)
  status, _headers, body = Main.run_rack(
    "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
    "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
  )
  text = +""
  body.each { |part| text << part }
  raise "GET #{path} answered #{status}: #{text}" unless status == 200
  text
end
"#;

#[test]
fn a_strict_local_named_like_a_helper_reads_the_local() {
    let run = app().run_ruby(&format!(
        r#"{GET}
text = get("/strict")
raise "a bare read became the helper: #{{text}}" unless text.include?("<p id=\"read\">S1|S2|S3|S4</p>")
raise "a read under || became the helper: #{{text}}" unless text.include?("<p id=\"or\">S1|S2|S3|S4</p>")
raise "empty parens read the local: #{{text}}" unless text.include?("<p id=\"call\">{META}")
puts "strict locals passed"
"#
    ));
    run.assert_passes();
    assert!(run.stdout.contains("strict locals passed"), "{}", run.stdout);
}

#[test]
fn a_render_local_named_like_a_helper_reads_the_local() {
    let run = app().run_ruby(&format!(
        r#"{GET}
text = get("/plain")
raise "a bare read became the helper: #{{text}}" unless text.include?("<p id=\"read\">P1|P2|P3|P4</p>")
raise "a read under || became the helper: #{{text}}" unless text.include?("<p id=\"or\">P1|P2|P3|P4</p>")
raise "empty parens read the local: #{{text}}" unless text.include?("<p id=\"call\">{META}")
puts "render locals passed"
"#
    ));
    run.assert_passes();
    assert!(run.stdout.contains("render locals passed"), "{}", run.stdout);
}

#[test]
fn an_ivar_named_like_a_helper_leaves_the_bare_name_the_helper() {
    let run = app().run_ruby(&format!(
        r#"{GET}
text = get("/ivar")
raise "the view read the ivar for the helper: #{{text}}" unless text.include?("<p id=\"view\">I1|{META}")
raise "the partial read the ivar for the helper: #{{text}}" unless text.include?("<p id=\"partial\">I1|{META}")
puts "ivar passed"
"#
    ));
    run.assert_passes();
    assert!(run.stdout.contains("ivar passed"), "{}", run.stdout);
}
