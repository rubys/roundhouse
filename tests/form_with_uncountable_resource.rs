//! `form_with` on an uncountable resource (`news`) posts a new record to
//! the collection route, whose helper is `news_index_path`, and a
//! persisted one to `news_path(id)`. The same holds under a namespace,
//! nested under a parent, and through `url: record`.
//!
//! Expected actions are Rails 8.1.4's (oracle `form-oracle.rb`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ROUTES: &str = "Rails.application.routes.draw do\n  resources :accounts, only: [:show] do\n    resources :news\n  end\n  resources :news\n  namespace :admin do\n    resources :news\n  end\nend\n";

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"accounts\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"news\", force: :cascade do |t|\n    t.string \"title\"\n    t.integer \"account_id\"\n  end\nend\n";

const ASSERTIONS: &str = r#"
fresh = News.new
account = Account.create!(id: 7, name: "a")
saved = News.create!(id: 3, title: "t", account_id: 7)
action = ->(html) { html[/action="([^"]*)"/, 1] }
got = [
  action.(Views::News.form(fresh)),
  action.(Views::News.form(saved)),
  action.(Views::News.admin_form(fresh)),
  action.(Views::News.admin_form(saved)),
  action.(Views::News.nested_form(fresh, nil, nil, account)),
  action.(Views::News.url_form(fresh)),
  action.(Views::News.url_form(saved))
]
want = ["/news", "/news/3", "/admin/news", "/admin/news/3", "/accounts/7/news", "/news", "/news/3"]
raise got.zip(want).reject { |g, w| g == w }.inspect unless got == want
puts "uncountable forms passed"
"#;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/account.rb", "class Account < ApplicationRecord\nend\n")
        .write("app/models/news.rb", "class News < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write(
            "app/controllers/news_controller.rb",
            "class NewsController < ApplicationController\n  def forms\n    @news = News.new\n    @account = Account.new\n  end\nend\n",
        )
        .write("config/routes.rb", ROUTES)
        .write("app/views/news/_form.html.erb", "<%= form_with model: news do |f| %><% end %>\n")
        .write("app/views/news/_admin_form.html.erb", "<%= form_with model: [:admin, news] do |f| %><% end %>\n")
        .write("app/views/news/_nested_form.html.erb", "<%= form_with model: [account, News.new] do |f| %><% end %>\n")
        // The render sites give the partials their locals.
        .write(
            "app/views/news/forms.html.erb",
            "<%= render \"form\", news: @news %>\n<%= render \"admin_form\", news: @news %>\n<%= render \"nested_form\", account: @account %>\n<%= render \"url_form\", news: @news %>\n",
        )
        .write("app/views/news/_url_form.html.erb", "<%= form_with model: news, url: news do |f| %><% end %>\n")
}

#[test]
fn an_uncountable_resource_form_posts_to_its_collection_route_on_ruby() {
    let run = example().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("uncountable forms passed"), "{}", run.stdout);
}
