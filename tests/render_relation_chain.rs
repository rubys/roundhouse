//! `render <relation chain>` in a view: Rails renders each record
//! through its class's partial, in the relation's order, and answers
//! nil for an empty collection, so `render(x) || "empty"` shows the
//! fallback. The chain names no partial (`order`, `limit` and a scope
//! are not models); the analyzed element class does.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ACTIONS: &[(&str, &str, &str)] = &[
    ("by_title", "@author = Author.first\n    @marker = \"*\"", "<%= render @author.posts.order(:title) %>"),
    ("published_two", "@author = Author.first", "<%= render @author.posts.published.limit(2) %>"),
    (
        "newest_two",
        "@author = Author.first",
        "<%= render @author.posts.includes(:author).order(title: :desc, id: :desc).limit(2) %>",
    ),
    ("latest", "@author = Author.first", "<%= render @author.posts.latest_first %>"),
    ("none", "@author = Author.first", "<%= render(@author.posts.where(title: \"none\")) || \"empty\" %>"),
    ("filled", "@author = Author.first", "<%= render(@author.posts.order(:title)) || \"empty\" %>"),
    ("nothing_listed", "@posts = Post.where(title: \"none\")", "<%= render(@posts) || \"<none>\" %>"),
    (
        "counted_none",
        "@author = Author.first",
        "<%= render(@author.counted_posts.where(title: \"none\")) || \"empty\" %><%= @author.lookups %>",
    ),
    (
        "counted_filled",
        "@author = Author.first",
        "<%= render(@author.counted_posts.order(:title)) || \"empty\" %><%= @author.lookups %>",
    ),
    ("listed", "@posts = Post.order(:title)", "<%= render @posts %>"),
    ("all_posts", "@author = Author.first", "<%= render @author.posts %>"),
];

fn app() -> emit_and_run::Overlay {
    let mut routes = String::from("Rails.application.routes.draw do\n");
    let mut controller = String::from("class AuthorsController < ApplicationController\n");
    let mut overlay = emit_and_run::empty_app();
    for (action, body, render) in ACTIONS {
        routes.push_str(&format!("  get \"/{action}\", to: \"authors#{action}\"\n"));
        controller.push_str(&format!("  def {action}\n    {body}\n  end\n\n"));
        overlay = overlay.write(&format!("app/views/authors/{action}.html.erb"), &format!("<ul>{render}</ul>\n"));
    }
    routes.push_str("end\n");
    controller.push_str("end\n");
    overlay
        .write("config/routes.rb", &routes)
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/authors_controller.rb", &controller)
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"authors\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n    t.boolean \"published\", default: false\n    t.bigint \"author_id\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "app/models/author.rb",
            "class Author < ApplicationRecord\n  has_many :posts\n\n  def counted_posts\n    @lookups = lookups + 1\n    Post.where(author_id: id)\n  end\n\n  def lookups\n    @lookups || 0\n  end\nend\n",
        )
        .write(
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  belongs_to :author\n  scope :published, -> { where(published: true) }\n\n  def self.latest_first\n    order(id: :desc)\n  end\nend\n",
        )
        .write("app/views/posts/_post.html.erb", "<li><%= @marker %><%= post.title %></li>\n")
}

const SEED: &str = r#"author = Author.create!(name: "Ann")
Post.create!(title: "b", published: true, author_id: author.id)
Post.create!(title: "a", published: true, author_id: author.id)
Post.create!(title: "c", published: false, author_id: author.id)
Post.create!(title: "d", published: true, author_id: author.id)
"#;

/// What Rails renders for each action, in `ACTIONS` order.
const WANT: &str = r#"want = {
  "by_title" => "<ul><li>*a</li>\n<li>*b</li>\n<li>*c</li>\n<li>*d</li>\n</ul>\n",
  "published_two" => "<ul><li>b</li>\n<li>a</li>\n</ul>\n",
  "newest_two" => "<ul><li>d</li>\n<li>c</li>\n</ul>\n",
  "latest" => "<ul><li>d</li>\n<li>c</li>\n<li>a</li>\n<li>b</li>\n</ul>\n",
  "none" => "<ul>empty</ul>\n",
  "filled" => "<ul><li>a</li>\n<li>b</li>\n<li>c</li>\n<li>d</li>\n</ul>\n",
  "nothing_listed" => "<ul>&lt;none&gt;</ul>\n",
  "counted_none" => "<ul>empty1</ul>\n",
  "counted_filled" => "<ul><li>a</li>\n<li>b</li>\n<li>c</li>\n<li>d</li>\n1</ul>\n",
  "listed" => "<ul><li>a</li>\n<li>b</li>\n<li>c</li>\n<li>d</li>\n</ul>\n",
  "all_posts" => "<ul><li>b</li>\n<li>a</li>\n<li>c</li>\n<li>d</li>\n</ul>\n",
}
"#;

#[test]
fn a_rendered_relation_chain_renders_each_record_in_order_on_ruby() {
    let script = format!(
        r##"{SEED}{WANT}got = want.keys.to_h do |action|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/#{{action}}", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  [action, "#{{status}} #{{body.respond_to?(:join) ? body.join : body}}"]
end
want.each do |action, html|
  raise "#{{action}}: #{{got[action].inspect}}" unless got[action] == "200 #{{html}}"
end
puts "relation chain renders passed"
"##
    );
    let run = app().run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains("relation chain renders passed"), "{}", run.stdout);
}

#[test]
fn the_renders_that_already_worked_emit_unchanged() {
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let view = |name: &str| std::fs::read_to_string(emitted.join(format!("app/views/authors/{name}.rb"))).unwrap();
    assert!(
        view("listed").contains("      posts.each { |p| Views::Posts.post_into(io, p, marker, notice, alert) }\n"),
        "{}",
        view("listed")
    );
    assert!(
        view("all_posts").contains("      author.posts.each { |p| Views::Posts.post_into(io, p, marker, notice, alert) }\n"),
        "{}",
        view("all_posts")
    );
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_rendered_relation_chain_renders_each_record_in_order_natively() {
    let script = format!(
        r##"{SEED}{WANT}empty = ActiveRecord::Relation.new(Post).where(title: "none")
by_title = ActiveRecord::Relation.new(Post).order(:title)
got = {{
  "by_title" => Views::Authors.by_title(author, "*"),
  "published_two" => Views::Authors.published_two(author, nil),
  "newest_two" => Views::Authors.newest_two(author, nil),
  "latest" => Views::Authors.latest(author, nil),
  "none" => Views::Authors.none(author, nil),
  "filled" => Views::Authors.filled(author, nil),
  "nothing_listed" => Views::Authors.nothing_listed(nil, empty),
  "counted_none" => Views::Authors.counted_none(Author.find(author.id), nil),
  "counted_filled" => Views::Authors.counted_filled(Author.find(author.id), nil),
  "listed" => Views::Authors.listed(nil, by_title),
  "all_posts" => Views::Authors.all_posts(author, nil),
}}
want.each do |action, html|
  raise "#{{action}}: #{{got[action].inspect}}" unless got[action] == html
end
puts "relation chain renders passed"
"##
    );
    let run = app().run_spinel_with_db(&script);
    run.assert_passes();
    assert!(run.stdout.contains("relation chain renders passed"), "{}", run.stdout);
}
