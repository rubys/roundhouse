//! One Representable contract shared by the interpreted and native output
//! lanes: the decorators, the route and controller that render one, and
//! what representable 3.2.0 itself renders for the same records.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "app/representers/comment_representer.rb",
            "class CommentRepresenter < Representable::Decorator\n  include Representable::JSON\n\n  property :commenter\n  property :body, as: :text\nend\n",
        )
        .write(
            "app/representers/article_representer.rb",
            r##"class ArticleRepresenter < Representable::Decorator
  include Representable::JSON

  property :id
  property :title, getter: ->(**) { title.upcase }
  property :summary, getter: ->(represented:, **) { represented.body.to_s[0, 7] }
  property :missing, getter: ->(**) { nil }, render_nil: true
  property :skipped, getter: ->(**) { nil }
  property :headline, exec_context: :decorator
  collection :comments, extend: CommentRepresenter

  def headline
    "#{represented.title}!"
  end
end
"##,
        )
        .write(
            "app/controllers/article_json_controller.rb",
            "class ArticleJsonController < ApplicationController\n  def show\n    article = Article.find(params[:id])\n    render json: ArticleRepresenter.new(article).to_hash\n  end\nend\n",
        )
        .edit("config/routes.rb", "  resources :articles do", "  get \"article_json/:id\" => \"article_json#show\"\n  resources :articles do")
}

pub const ASSERTIONS: &str = r#"
article = Article.create!(title: "Hello", body: "Rails is great")
Comment.create!(article_id: article.id, commenter: "Ada", body: "First!")
Comment.create!(article_id: article.id, commenter: "Linus", body: "Second")
controller = ArticleJsonController.new
controller.params = {"id" => article.id.to_s}
controller.process_action(:show)
expected = '{"id":' + article.id.to_s + ',"title":"HELLO","summary":"Rails i","missing":null,"headline":"Hello!","comments":[{"commenter":"Ada","text":"First!"},{"commenter":"Linus","text":"Second"}]}'
raise controller.body.inspect unless controller.body == expected
raise controller.content_type.inspect unless controller.content_type == "application/json"
hash = ArticleRepresenter.new(article).to_hash
raise hash.inspect unless hash["title"] == "HELLO" && !hash.key?("skipped") && hash.key?("missing")
puts "Representable decorator OK"
"#;
