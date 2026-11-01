//! A partial rendered without an object reads its caller's ivar named
//! like its directory's singular (`@post` in `posts/_summary`). Rails
//! renders a partial in its caller's view context, so the ivar is the
//! action's, wherever it was set: in the action, a `before_action`, a
//! memoizing method. The partial's record param stands for that ivar, so
//! a render that hands it nothing passes the caller's own; it used to
//! pass nil. A partial some render does hand a record (`post: other`)
//! keeps the two apart, and passes the ivar on, not the record.

use super::emit_and_run;

const POSTS: &str = r#"class PostsController < ApplicationController
  before_action :set_post, only: [:preview]

  def show
    @post = Post.find_by(title: "Rails tips")
  end

  def preview
  end

  def memoized
    current_post
  end

  def nested
    @post = Post.find_by(title: "Rails tips")
  end

  def prefixed
    @post = Post.find_by(title: "Rails tips")
  end

  def helped
    @post = Post.find_by(title: "Rails tips")
  end

  def rebound
    @post = Post.find_by(title: "Rails tips")
    @other = Post.find_by(title: "Go tips")
  end

  private

  def set_post
    @post = Post.find_by(title: "Rails tips")
  end

  def current_post
    @post ||= Post.find_by(title: "Rails tips")
  end
end
"#;

const REPORTS: &str = r#"class ReportsController < ApplicationController
  def show
    @post = Post.find_by(title: "Rails tips")
    @report = Post.find_by(title: "Go tips")
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  get "/posts/show", to: "posts#show"
  get "/posts/preview", to: "posts#preview"
  get "/posts/memoized", to: "posts#memoized"
  get "/posts/nested", to: "posts#nested"
  get "/posts/prefixed", to: "posts#prefixed"
  get "/posts/helped", to: "posts#helped"
  get "/posts/rebound", to: "posts#rebound"
  get "/reports/show", to: "reports#show"
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :posts do |t|\n    t.string :title\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/post.rb", "class Post < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("app/controllers/posts_controller.rb", POSTS)
        .write("app/controllers/reports_controller.rb", REPORTS)
        .write(
            "app/helpers/posts_helper.rb",
            "module PostsHelper\n  def summary_html\n    render \"posts/summary\"\n  end\nend\n",
        )
        .write("config/routes.rb", ROUTES)
        .write("app/views/layouts/application.html.erb", "<main><%= yield %></main>\n")
        .write("app/views/posts/_summary.html.erb", "<p class=\"summary\"><%= @post.title %></p>\n")
        .write("app/views/posts/_outer.html.erb", "<section><%= render partial: \"summary\" %></section>\n")
        .write("app/views/posts/_prefixed.html.erb", "<p class=\"prefixed\"><%= prefix %> <%= @post.title %></p>\n")
        .write("app/views/posts/show.html.erb", "<div><%= render \"summary\" %></div>\n")
        .write("app/views/posts/preview.html.erb", "<div><%= render partial: \"summary\" %></div>\n")
        .write("app/views/posts/memoized.html.erb", "<div><%= render \"posts/summary\" %></div>\n")
        .write("app/views/posts/nested.html.erb", "<div><%= render \"outer\" %></div>\n")
        .write(
            "app/views/posts/prefixed.html.erb",
            "<div><%= render partial: \"prefixed\", locals: { prefix: \"New:\" } %></div>\n",
        )
        .write("app/views/posts/helped.html.erb", "<div><%= summary_html %></div>\n")
        .write("app/views/posts/_framed.html.erb", "<section><%= post.title %><%= render \"summary\" %></section>\n")
        .write(
            "app/views/posts/rebound.html.erb",
            "<div><%= render \"framed\", post: @other %><%= render partial: \"framed\", locals: { post: @other } %></div>\n",
        )
        .write("app/views/reports/_box.html.erb", "<u><%= @report.title %></u>\n")
        .write("app/views/reports/show.html.erb", "<div><%= render \"posts/summary\" %><%= render \"box\" %></div>\n")
}

const ASSERTIONS: &str = r#"
require_relative "app/controllers/posts_controller"
require_relative "app/controllers/reports_controller"

Post.create!(title: "Rails tips")
Post.create!(title: "Go tips")

def body_of(c, action)
  c.params = {}
  ActionController::Current.controller = c
  c.process_action(action)
  c.body
rescue NoMethodError => e
  e.message
end

summary = "<p class=\"summary\">Rails tips</p>"
missing = []
{
  show: [summary],
  preview: [summary],
  memoized: [summary],
  nested: ["<section>#{summary}"],
  prefixed: ["<p class=\"prefixed\">New: Rails tips</p>"],
  helped: [summary],
}.each do |action, wants|
  body = body_of(PostsController.new, action)
  missing << "posts##{action}: #{body}" unless wants.all? { |want| body.include?(want) }
end
rebound = body_of(PostsController.new, :rebound)
missing << "posts#rebound: #{rebound}" unless rebound.scan("<section>Go tips#{summary}").size == 2
report = body_of(ReportsController.new, :show)
missing << "reports#show: #{report}" unless [summary, "<u>Go tips</u>"].all? { |want| report.include?(want) }
raise missing.join("\n") unless missing.empty?
puts "every partial read its caller's ivar"
"#;

#[test]
fn a_partial_without_an_object_reads_its_callers_ivar() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("every partial read its caller's ivar"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_partial_without_an_object_reads_its_callers_ivar_on_spinel() {
    let run = app().run_spinel_with_db(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("every partial read its caller's ivar"), "{}", run.stdout);
}
