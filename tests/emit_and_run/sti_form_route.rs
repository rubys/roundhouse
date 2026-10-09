//! `form_with model: record` on an STI base routes by the record's
//! CLASS, as Rails' `polymorphic_path` does: a `Articles::Featured` row
//! edited through the shared form posts to `/articles/featureds/:id`,
//! while a plain `Article` keeps `/articles/:id`. Hydration is
//! base-classed, so the lowered action dispatches on the type column.

use super::emit_and_run;

#[test]
fn sti_subclass_form_action_follows_the_records_class() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.string \"title\"\n    t.text \"body\"\n",
            "    t.string \"title\"\n    t.string \"type\"\n    t.text \"body\"\n",
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  namespace :articles do\n    resources :featureds, only: :show\n  end\n\n  resources :articles do",
        )
        .write("app/models/articles/featured.rb", "class Articles::Featured < Article\nend\n")
        .write(
            "app/controllers/articles/featureds_controller.rb",
            "class Articles::FeaturedsController < ApplicationController\n  def show\n    head :ok\n  end\nend\n",
        )
        .run_ruby(
            r#"
require "stringio"
body = "A sufficiently long article body."
plain = Article.create!(title: "Plain", body: body)
featured = Articles::Featured.create!(title: "Featured", body: body)

def get(path)
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
          "HTTP_ACCEPT" => "text/html" }
  status, html = Main.dispatch_core(env, StringIO.new(""))
  [status, html.to_s]
end

status, html = get("/articles/#{plain.id}/edit")
raise "plain edit #{status}" unless status == 200
raise "plain article must post to /articles/#{plain.id}: #{html[/<form[^>]*>/]}" unless html.include?(%(action="/articles/#{plain.id}"))

status, html = get("/articles/#{featured.id}/edit")
raise "featured edit #{status}" unless status == 200
raise "featured article must post to /articles/featureds/#{featured.id}: #{html[/<form[^>]*>/]}" unless html.include?(%(action="/articles/featureds/#{featured.id}"))

status, html = get("/articles/new")
raise "new article #{status}" unless status == 200
raise "new plain article must post to /articles: #{html[/<form[^>]*>/]}" unless html.include?(%(action="/articles"))
puts "sti form routes OK"
"#,
        )
        .assert_passes();
}
