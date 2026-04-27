require_relative "application_record"
require_relative "../../runtime/broadcasts"
require_relative "../views/comments/_comment"
require_relative "../views/articles/_article"

# Lowered shape: real-blog's `Comment < ApplicationRecord` with
# `belongs_to :article`, presence validations on commenter and body,
# plus broadcasts (deferred).
#
# Same structural pattern as Article — explicit per-column accessors,
# typed initialize/attributes/[]/[]=/update, block-attribute validations.
# `belongs_to :article` becomes a typed `article` method returning
# `Article | nil`.
class Comment < ApplicationRecord
  attr_accessor :article_id, :commenter, :body, :created_at, :updated_at

  def self.table_name
    "comments"
  end

  def self.schema_columns
    [:id, :article_id, :commenter, :body, :created_at, :updated_at]
  end

  def self.instantiate(row)
    instance = new(row)
    instance.mark_persisted!
    instance
  end

  def initialize(attrs = {})
    super()
    self.id         = attrs[:id]         || 0
    self.article_id = attrs[:article_id] || 0
    self.commenter  = attrs[:commenter]
    self.body       = attrs[:body]
    self.created_at = attrs[:created_at]
    self.updated_at = attrs[:updated_at]
  end

  def attributes
    {
      article_id: @article_id,
      commenter:  @commenter,
      body:       @body,
      created_at: @created_at,
      updated_at: @updated_at,
    }
  end

  def [](name)
    case name
    when :id         then @id
    when :article_id then @article_id
    when :commenter  then @commenter
    when :body       then @body
    when :created_at then @created_at
    when :updated_at then @updated_at
    end
  end

  def []=(name, value)
    case name
    when :id         then @id = value
    when :article_id then @article_id = value
    when :commenter  then @commenter = value
    when :body       then @body = value
    when :created_at then @created_at = value
    when :updated_at then @updated_at = value
    end
  end

  def update(attrs)
    self.article_id = attrs[:article_id] if attrs.key?(:article_id)
    self.commenter  = attrs[:commenter]  if attrs.key?(:commenter)
    self.body       = attrs[:body]       if attrs.key?(:body)
    self.created_at = attrs[:created_at] if attrs.key?(:created_at)
    self.updated_at = attrs[:updated_at] if attrs.key?(:updated_at)
    save
  end

  def validate
    validates_presence_of(:commenter) { @commenter }
    validates_presence_of(:body)      { @body }
  end

  # belongs_to :article  → typed accessor; nil when FK doesn't resolve.
  def article
    @article_id == 0 ? nil : Article.find_by(id: @article_id)
  end

  # broadcasts_to ->(comment) { "article_<id>_comments" }, target: "comments"
  #   → append partial to "comments" target on create
  #   → replace partial at "comment_<id>" target on update
  #   → remove element at "comment_<id>" target on destroy
  #
  # Plus the explicit re-render-parent hooks from real-blog: each
  # create/destroy of a comment also replaces the parent article on
  # the index-page "articles" stream, so the comment-count display
  # in the article partial stays current.
  def after_create_commit
    Broadcasts.append(
      stream: "article_#{@article_id}_comments",
      target: "comments",
      html: Views::Comments.comment(self),
    )
    parent = article
    return if parent.nil?
    Broadcasts.replace(
      stream: "articles",
      target: "article_#{parent.id}",
      html: Views::Articles.article(parent),
    )
  end

  def after_update_commit
    Broadcasts.replace(
      stream: "article_#{@article_id}_comments",
      target: "comment_#{@id}",
      html: Views::Comments.comment(self),
    )
  end

  def after_destroy_commit
    Broadcasts.remove(
      stream: "article_#{@article_id}_comments",
      target: "comment_#{@id}",
    )
    parent = article
    return if parent.nil?
    Broadcasts.replace(
      stream: "articles",
      target: "article_#{parent.id}",
      html: Views::Articles.article(parent),
    )
  end
end
