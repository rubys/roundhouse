//! One ActiveModel::Serializer contract shared by the interpreted and
//! native lanes: the serializers, the controller that renders through
//! them, and what active_model_serializers 0.10.16 itself renders for the
//! same records (oracle `attributes.rb`).

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "app/serializers/article_serializer.rb",
            "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id, :title\nend\n",
        )
        .write(
            "app/serializers/full_article_serializer.rb",
            r##"class FullArticleSerializer < ActiveModel::Serializer
  attributes :id, :title, :summary
  attribute :body, key: :text
  attribute(:shout) { object.title.upcase }
  attribute :draft_note, if: :draft?
  attribute :public_note, unless: :draft?

  def summary
    object.body.to_s[0, 5]
  end

  def draft?
    object.title.start_with?("Draft")
  end

  def draft_note
    "draft"
  end

  def public_note
    "public"
  end
end
"##,
        )
        .write(
            "app/serializers/guarded_article_serializer.rb",
            r##"class GuardedArticleSerializer < ActiveModel::Serializer
  attributes :id, :long?
  attribute(:shout, if: :long?) { object.body.upcase }
  attribute(:never, if: :never?) { Integer(object.title) }
  attribute(:short_note, unless: :long?) { "short" }

  def long?
    object.body.to_s.length > 10
  end

  def never?
    false
  end
end
"##,
        )
        .write(
            "app/controllers/article_json_controller.rb",
            "class ArticleJsonController < ApplicationController\n  def show\n    render json: Article.find(params[:id]), serializer: FullArticleSerializer\n  end\n\n  def index\n    render json: Article.order(:id), each_serializer: ArticleSerializer\n  end\n\n  def guarded\n    render json: Article.find(params[:id]), serializer: GuardedArticleSerializer\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"article_json\" => \"article_json#index\"\n  get \"article_json/:id/guarded\" => \"article_json#guarded\"\n  get \"article_json/:id\" => \"article_json#show\"\n  resources :articles do",
        )
}

pub const ASSERTIONS: &str = r#"
a = Article.create!(title: "Hello", body: "A long body")
d = Article.create!(title: "Draft one", body: "A long draft")
show = ->(id) { c = ArticleJsonController.new; c.params = {"id" => id.to_s}; c.process_action(:show); c }
c = show.(a.id)
expected = '{"id":' + a.id.to_s + ',"title":"Hello","summary":"A lon","text":"A long body","shout":"HELLO","public_note":"public"}'
raise c.body.inspect unless c.body == expected
raise c.content_type.inspect unless c.content_type == "application/json; charset=utf-8"
c = show.(d.id)
expected = '{"id":' + d.id.to_s + ',"title":"Draft one","summary":"A lon","text":"A long draft","shout":"DRAFT ONE","draft_note":"draft"}'
raise c.body.inspect unless c.body == expected
list = ArticleJsonController.new
list.params = {}
list.process_action(:index)
expected = "[" + Article.order(:id).map { |r| '{"id":' + r.id.to_s + ',"title":' + r.title.inspect + '}' }.join(",") + "]"
raise list.body.inspect unless list.body == expected
# `if:` / `unless:`: the reader runs only when the condition holds (the
# `never` block raises if it runs), and a `?` attribute keeps its `?`.
guarded = ->(id) { c = ArticleJsonController.new; c.params = {"id" => id.to_s}; c.process_action(:guarded); c.body }
expected = '{"id":' + a.id.to_s + ',"long?":true,"shout":"A LONG BODY"}'
raise guarded.(a.id).inspect unless guarded.(a.id) == expected
s = Article.create!(title: "Short", body: "tiny body!")
expected = '{"id":' + s.id.to_s + ',"long?":false,"short_note":"short"}'
raise guarded.(s.id).inspect unless guarded.(s.id) == expected
puts "ActiveModel::Serializer OK"
"#;
