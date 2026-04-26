require_relative "application_record"

# Lowered shape: real-blog's `Article < ApplicationRecord` with
# `has_many :comments, dependent: :destroy`, `validates :title, presence: true`,
# `validates :body, presence: true, length: { minimum: 10 }`, plus broadcasts.
#
# Per-column accessors via `attr_accessor` (built-in Ruby — fine in spinel),
# typed `initialize` / `attributes` / `[]` / `[]=` / `update` instead of
# reflective `attr_accessor`-override + `attrs.each { send }`. Validations
# expressed as direct helper calls in the `validate` method, with block-based
# attribute access. Association expanded into a typed `comments` method.
# Broadcasts not yet wired (deferred to a later iteration).
class Article < ApplicationRecord
  attr_accessor :title, :body, :created_at, :updated_at

  def self.table_name
    "articles"
  end

  def self.schema_columns
    [:id, :title, :body, :created_at, :updated_at]
  end

  def self.instantiate(row)
    instance = new(row)
    instance.mark_persisted!
    instance
  end

  def initialize(attrs = {})
    super()
    self.id         = attrs[:id]         || 0
    self.title      = attrs[:title]
    self.body       = attrs[:body]
    self.created_at = attrs[:created_at]
    self.updated_at = attrs[:updated_at]
  end

  def attributes
    {
      title:      @title,
      body:       @body,
      created_at: @created_at,
      updated_at: @updated_at,
    }
  end

  def [](name)
    case name
    when :id         then @id
    when :title      then @title
    when :body       then @body
    when :created_at then @created_at
    when :updated_at then @updated_at
    end
  end

  def []=(name, value)
    case name
    when :id         then @id = value
    when :title      then @title = value
    when :body       then @body = value
    when :created_at then @created_at = value
    when :updated_at then @updated_at = value
    end
  end

  def update(attrs)
    self.title      = attrs[:title]      if attrs.key?(:title)
    self.body       = attrs[:body]       if attrs.key?(:body)
    self.created_at = attrs[:created_at] if attrs.key?(:created_at)
    self.updated_at = attrs[:updated_at] if attrs.key?(:updated_at)
    save
  end

  def validate
    validates_presence_of(:title) { @title }
    validates_presence_of(:body)  { @body }
    validates_length_of(:body, minimum: 10) { @body }
  end

  # has_many :comments  → typed accessor returning Array<Comment>.
  def comments
    Comment.where(article_id: @id)
  end

  # has_many :comments, dependent: :destroy → cascade in before_destroy.
  def before_destroy
    comments.each { |c| c.destroy }
  end
end
