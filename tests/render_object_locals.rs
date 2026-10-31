//! `render @posts, flag: "no"` in a view: for any first argument but a
//! Hash, Rails renders the object's partial with the hash as its
//! locals, once per record for a collection, and every key is a local
//! (`as:` included). The same calls without the hash already rendered;
//! with it they stayed a bare `render`. A strict-locals partial takes
//! the locals as keywords, from this form and from an explicit
//! `collection:` with `locals:` alike. Rails evaluates the hash once,
//! after the collection and before any record renders, so a local
//! with a side effect (`(n += 1).to_s`) runs once for a collection of
//! three and once for an empty one, and one that reassigns the
//! collection's local leaves the records it renders alone. A single
//! render, record or named, runs its locals in source order, which is
//! not the order of the partial's params.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ACTIONS: &[(&str, &str, &str)] = &[
    ("listed", "@posts = Post.order(:title)", "<%= render @posts, flag: \"no\" %>"),
    ("by_author", "@author = Author.first\n    @marker = \"*\"", "<%= render @author.posts, flag: @marker %>"),
    ("by_title", "@author = Author.first", "<%= render @author.posts.order(:title), flag: \"yes\" %>"),
    ("first", "@post = Post.order(:title).first", "<%= render @post, flag: \"one\" %>"),
    ("none", "@posts = Post.where(title: \"none\")", "<%= render(@posts, flag: \"no\") || \"empty\" %>"),
    ("filled", "@posts = Post.order(:title)", "<%= render(@posts, flag: \"no\") || \"empty\" %>"),
    ("as_local", "@posts = Post.order(:title)", "<%= render @posts, flag: \"as\", as: :item %>"),
    ("comments", "@comments = Comment.order(:body)", "<%= render @comments %>"),
    ("articles", "@articles = Article.order(:title)", "<%= render @articles, mark: \"+\" %>"),
    ("article", "@article = Article.order(:title).first", "<%= render @article, mark: \"+\" %>"),
    (
        "named_articles",
        "@articles = Article.order(:title)",
        "<%= render partial: \"articles/article\", collection: @articles, locals: { mark: \"!\" } %>",
    ),
    ("plain_articles", "@articles = Article.order(:title)", "<%= render @articles %>"),
    (
        "once_listed",
        "@posts = Post.order(:title)",
        "<% n = 0 %><%= render @posts, flag: (n += 1).to_s, as: \"kept\" %>|<%= n %>",
    ),
    (
        "once_listed_empty",
        "@posts = Post.where(title: \"none\")",
        "<% n = 0 %><%= render @posts, flag: (n += 1).to_s, as: \"kept\" %>|<%= n %>",
    ),
    ("once_by_author", "@author = Author.first", "<% n = 0 %><%= render @author.posts, flag: (n += 1).to_s %>|<%= n %>"),
    (
        "once_by_title",
        "@author = Author.first",
        "<% n = 0 %><%= render @author.posts.order(:title), flag: (n += 1).to_s %>|<%= n %>",
    ),
    (
        "once_by_title_empty",
        "@author = Author.first",
        "<% n = 0 %><%= render @author.posts.where(title: \"none\"), flag: (n += 1).to_s %>|<%= n %>",
    ),
    (
        "once_named",
        "@posts = Post.order(:title)",
        "<% n = 0 %><%= render partial: \"posts/post\", collection: @posts, locals: { flag: (n += 1).to_s } %>|<%= n %>",
    ),
    (
        "once_named_empty",
        "@posts = Post.where(title: \"none\")",
        "<% n = 0 %><%= render partial: \"posts/post\", collection: @posts, locals: { flag: (n += 1).to_s } %>|<%= n %>",
    ),
    ("once_or", "@posts = Post.order(:title)", "<% n = 0 %><%= render(@posts, flag: (n += 1).to_s) || \"empty\" %>|<%= n %>"),
    (
        "once_or_empty",
        "@posts = Post.where(title: \"none\")",
        "<% n = 0 %><%= render(@posts, flag: (n += 1).to_s) || \"empty\" %>|<%= n %>",
    ),
    ("once_articles", "@articles = Article.order(:title)", "<% n = 0 %><%= render @articles, mark: (n += 1).to_s %>|<%= n %>"),
    (
        "once_articles_empty",
        "@articles = Article.where(title: \"none\")",
        "<% n = 0 %><%= render @articles, mark: (n += 1).to_s %>|<%= n %>",
    ),
    (
        "once_in_order",
        "@posts = Post.order(:title)",
        "<% n = 0 %><%= render @posts, flag: (n += 1).to_s, as: (n *= 10).to_s %>|<%= n %>",
    ),
    (
        "once_after_collection",
        "@author = Author.first",
        "<% n = 0 %><%= render @author.posts.limit(n += 1), flag: (n *= 10).to_s %>|<%= n %>",
    ),
    (
        "reassigned",
        "@posts = Post.order(:title)",
        "<% posts = @posts %><%= render posts, flag: (posts = Post.where(title: \"c\")).size.to_s %>|<%= posts.size %>",
    ),
    (
        "reassigned_empty",
        "@posts = Post.where(title: \"none\")",
        "<% posts = @posts %><%= render posts, flag: (posts = Post.where(title: \"c\")).size.to_s %>|<%= posts.size %>",
    ),
    (
        "first_in_order",
        "@post = Post.order(:title).first",
        "<% n = 0 %><%= render @post, flag: (n += 1).to_s, as: (n *= 10).to_s %>|<%= n %>",
    ),
    (
        "article_in_order",
        "@article = Article.order(:title).first",
        "<% n = 0 %><%= render @article, tail: (n += 1).to_s, mark: (n *= 10).to_s %>|<%= n %>",
    ),
    (
        "named_in_order",
        "@post = Post.order(:title).first",
        "<% n = 0 %><%= render \"posts/post\", post: @post, flag: (n += 1).to_s, as: (n *= 10).to_s %>|<%= n %>",
    ),
    (
        "named_article_in_order",
        "@article = Article.order(:title).first",
        "<% n = 0 %><%= render \"articles/article\", article: @article, tail: (n += 1).to_s, mark: (n *= 10).to_s %>|<%= n %>",
    ),    ("shadowed", "@posts = Post.order(:title)", "<% p = \"five\" %><%= render @posts, flag: p %>"),
    (
        "shadowed_named",
        "@posts = Post.order(:title)",
        "<% post = \"x\" %><%= render partial: \"posts/post\", collection: @posts, locals: { flag: post } %>",
    ),
    ("shadowed_strict", "@articles = Article.order(:title)", "<% a = \"!\" %><%= render @articles, mark: a %>"),
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
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"authors\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n    t.bigint \"author_id\"\n  end\n  create_table \"comments\", force: :cascade do |t|\n    t.string \"body\"\n    t.bigint \"author_id\"\n  end\n  create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/author.rb", "class Author < ApplicationRecord\n  has_many :posts\n  has_many :comments\nend\n")
        .write("app/models/post.rb", "class Post < ApplicationRecord\n  belongs_to :author\nend\n")
        .write("app/models/comment.rb", "class Comment < ApplicationRecord\n  belongs_to :author\nend\n")
        .write("app/views/posts/_post.html.erb", "<li><%= post.title %>:<%= flag %></li>\n")
        .write("app/views/comments/_comment.html.erb", "<li><%= comment.body %></li>\n")
        .write("app/models/article.rb", "class Article < ApplicationRecord\nend\n")
        .write(
            "app/views/articles/_article.html.erb",
            "<%# locals: (article:, mark: \"-\", tail: \"\") -%>\n<li><%= mark %><%= article.title %><%= tail %></li>\n",
        )
}

const SEED: &str = r#"author = Author.create!(name: "Ann")
Post.create!(title: "b", author_id: author.id)
Post.create!(title: "a", author_id: author.id)
Post.create!(title: "c", author_id: author.id)
Comment.create!(body: "y", author_id: author.id)
Comment.create!(body: "x", author_id: author.id)
Article.create!(title: "q")
Article.create!(title: "p")
"#;

/// What Rails renders for each action, in `ACTIONS` order.
const WANT: &str = r#"want = {
  "listed" => "<ul><li>a:no</li>\n<li>b:no</li>\n<li>c:no</li>\n</ul>\n",
  "by_author" => "<ul><li>b:*</li>\n<li>a:*</li>\n<li>c:*</li>\n</ul>\n",
  "by_title" => "<ul><li>a:yes</li>\n<li>b:yes</li>\n<li>c:yes</li>\n</ul>\n",
  "first" => "<ul><li>a:one</li>\n</ul>\n",
  "none" => "<ul>empty</ul>\n",
  "filled" => "<ul><li>a:no</li>\n<li>b:no</li>\n<li>c:no</li>\n</ul>\n",
  "as_local" => "<ul><li>a:as</li>\n<li>b:as</li>\n<li>c:as</li>\n</ul>\n",
  "comments" => "<ul><li>x</li>\n<li>y</li>\n</ul>\n",
  "articles" => "<ul><li>+p</li>\n<li>+q</li>\n</ul>\n",
  "article" => "<ul><li>+p</li>\n</ul>\n",
  "named_articles" => "<ul><li>!p</li>\n<li>!q</li>\n</ul>\n",
  "plain_articles" => "<ul><li>-p</li>\n<li>-q</li>\n</ul>\n",
  "once_listed" => "<ul><li>a:1</li>\n<li>b:1</li>\n<li>c:1</li>\n|1</ul>\n",
  "once_listed_empty" => "<ul>|1</ul>\n",
  "once_by_author" => "<ul><li>b:1</li>\n<li>a:1</li>\n<li>c:1</li>\n|1</ul>\n",
  "once_by_title" => "<ul><li>a:1</li>\n<li>b:1</li>\n<li>c:1</li>\n|1</ul>\n",
  "once_by_title_empty" => "<ul>|1</ul>\n",
  "once_named" => "<ul><li>a:1</li>\n<li>b:1</li>\n<li>c:1</li>\n|1</ul>\n",
  "once_named_empty" => "<ul>|1</ul>\n",
  "once_or" => "<ul><li>a:1</li>\n<li>b:1</li>\n<li>c:1</li>\n|1</ul>\n",
  "once_or_empty" => "<ul>empty|1</ul>\n",
  "once_articles" => "<ul><li>1p</li>\n<li>1q</li>\n|1</ul>\n",
  "once_articles_empty" => "<ul>|1</ul>\n",
  "once_in_order" => "<ul><li>a:1</li>\n<li>b:1</li>\n<li>c:1</li>\n|10</ul>\n",
  "once_after_collection" => "<ul><li>b:10</li>\n|10</ul>\n",
  "reassigned" => "<ul><li>a:1</li>\n<li>b:1</li>\n<li>c:1</li>\n|1</ul>\n",
  "reassigned_empty" => "<ul>|1</ul>\n",
  "first_in_order" => "<ul><li>a:1</li>\n|10</ul>\n",
  "article_in_order" => "<ul><li>10p1</li>\n|10</ul>\n",
  "named_in_order" => "<ul><li>a:1</li>\n|10</ul>\n",
  "named_article_in_order" => "<ul><li>10p1</li>\n|10</ul>\n",
  "shadowed" => "<ul><li>a:five</li>\n<li>b:five</li>\n<li>c:five</li>\n</ul>\n",
  "shadowed_named" => "<ul><li>a:x</li>\n<li>b:x</li>\n<li>c:x</li>\n</ul>\n",
  "shadowed_strict" => "<ul><li>!p</li>\n<li>!q</li>\n</ul>\n",
}
"#;

#[test]
fn an_object_render_passes_its_hash_as_the_partials_locals_on_ruby() {
    let script = format!(
        r##"{SEED}{WANT}got = want.keys.to_h do |action|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/#{{action}}", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  [action, "#{{status}} #{{body.respond_to?(:join) ? body.join : body}}"]
end
want.each do |action, html|
  raise "#{{action}}: #{{got[action].inspect}}" unless got[action] == "200 #{{html}}"
end
puts "object renders with locals passed"
"##
    );
    let run = app().run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains("object renders with locals passed"), "{}", run.stdout);
}

#[test]
fn an_object_render_with_locals_emits_the_partial_call() {
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let view = |name: &str| std::fs::read_to_string(emitted.join(format!("app/views/authors/{name}.rb"))).unwrap();
    assert!(
        view("listed").contains("      posts.each { |p| Views::Posts.post_into(io, p, notice, alert, nil, \"no\") }\n"),
        "{}",
        view("listed")
    );
    // Each line, after the one before it; the collection event may sit between them.
    let in_order = |text: &str, lines: &[&str]| {
        let mut rest = text;
        lines.iter().all(|line| match rest.find(line) {
            Some(at) => {
                rest = &rest[at + line.len()..];
                true
            }
            None => false,
        })
    };
    let once_listed = view("once_listed");
    assert!(
        in_order(
            &once_listed,
            &[
                "      __collection_26 = posts\n",
                "      __local_26_flag = (n += 1).to_s\n",
                "      __collection_26.each { |p| Views::Posts.post_into(io, p, notice, alert, \"kept\", __local_26_flag) }\n",
            ]
        ),
        "{once_listed}"
    );
    let first_in_order = view("first_in_order");
    assert!(
        in_order(
            &first_in_order,
            &[
                "      __record_26 = post\n",
                "      __local_26_flag = (n += 1).to_s\n",
                "      __local_26_as = (n *= 10).to_s\n",
                "      Views::Posts.post_into(io, __record_26, notice, alert, __local_26_as, __local_26_flag)\n",
            ]
        ),
        "{first_in_order}"
    );
    assert!(
        view("first").contains("      Views::Posts.post_into(io, post, notice, alert, nil, \"one\")\n"),
        "{}",
        view("first")
    );
    assert!(
        view("comments").contains("      comments.each { |c| Views::Comments.comment_into(io, c, notice, alert) }\n"),
        "{}",
        view("comments")
    );
    assert!(
        view("articles").contains("      articles.each { |a| io << Views::Articles.article(a, mark: \"+\") }\n"),
        "{}",
        view("articles")
    );
    assert!(
        view("plain_articles").contains("      articles.each { |a| io << Views::Articles.article(a) }\n"),
        "{}",
        view("plain_articles")
    );
}

#[test]
fn a_render_with_a_hash_that_is_not_a_literal_stays_bare() {
    let (emitted, errors) = app()
        .write("app/views/authors/listed.html.erb", "<% opts = { flag: \"no\" } %><ul><%= render @posts, opts %></ul>\n")
        .write("app/views/authors/first.html.erb", "<ul><%= render @post, \"flag\" => \"one\" %></ul>\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let view = |name: &str| std::fs::read_to_string(emitted.join(format!("app/views/authors/{name}.rb"))).unwrap();
    assert!(view("listed").contains("(render posts, opts)"), "{}", view("listed"));
    assert!(view("first").contains("(render post, \"flag\" => \"one\")"), "{}", view("first"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_object_render_passes_its_hash_as_the_partials_locals_natively() {
    let script = format!(
        r##"{SEED}{WANT}empty = ActiveRecord::Relation.new(Post).where(title: "none")
by_title = ActiveRecord::Relation.new(Post).order(:title)
articles = ActiveRecord::Relation.new(Article).order(:title)
got = {{
  "listed" => Views::Authors.listed(by_title),
  "by_author" => Views::Authors.by_author(author, "*"),
  "by_title" => Views::Authors.by_title(author),
  "first" => Views::Authors.first(by_title.first),
  "none" => Views::Authors.none(empty),
  "filled" => Views::Authors.filled(by_title),
  "as_local" => Views::Authors.as_local(by_title),
  "comments" => Views::Authors.comments(ActiveRecord::Relation.new(Comment).order(:body)),
  "articles" => Views::Authors.articles(articles),
  "article" => Views::Authors.article(articles.first),
  "named_articles" => Views::Authors.named_articles(articles),
  "plain_articles" => Views::Authors.plain_articles(articles),
  "once_listed" => Views::Authors.once_listed(by_title),
  "once_listed_empty" => Views::Authors.once_listed_empty(empty),
  "once_by_author" => Views::Authors.once_by_author(author),
  "once_by_title" => Views::Authors.once_by_title(author),
  "once_by_title_empty" => Views::Authors.once_by_title_empty(author),
  "once_named" => Views::Authors.once_named(by_title),
  "once_named_empty" => Views::Authors.once_named_empty(empty),
  "once_or" => Views::Authors.once_or(by_title),
  "once_or_empty" => Views::Authors.once_or_empty(empty),
  "once_articles" => Views::Authors.once_articles(articles),
  "once_articles_empty" => Views::Authors.once_articles_empty(ActiveRecord::Relation.new(Article).where(title: "none")),
  "once_in_order" => Views::Authors.once_in_order(by_title),
  "once_after_collection" => Views::Authors.once_after_collection(author),
  "reassigned" => Views::Authors.reassigned(by_title),
  "reassigned_empty" => Views::Authors.reassigned_empty(empty),
  "first_in_order" => Views::Authors.first_in_order(by_title.first),
  "article_in_order" => Views::Authors.article_in_order(articles.first),
  "named_in_order" => Views::Authors.named_in_order(by_title.first),
  "named_article_in_order" => Views::Authors.named_article_in_order(articles.first),
  "shadowed" => Views::Authors.shadowed(by_title),
  "shadowed_named" => Views::Authors.shadowed_named(by_title),
  "shadowed_strict" => Views::Authors.shadowed_strict(articles),
}}
want.each do |action, html|
  raise "#{{action}}: #{{got[action].inspect}}" unless got[action] == html
end
puts "object renders with locals passed"
"##
    );
    let run = app().run_spinel_with_db(&script);
    run.assert_passes();
    assert!(run.stdout.contains("object renders with locals passed"), "{}", run.stdout);
}
