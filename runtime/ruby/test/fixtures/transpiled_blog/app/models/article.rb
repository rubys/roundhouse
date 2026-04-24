# Transpiled shape of fixtures/real-blog/app/models/article.rb.
#
# Expansion applied (per ruby2js's filter/rails/model.rb, updated
# for a typed-field-per-attribute representation):
#   - schema columns: declared via `attr_accessor` — tells the transpile
#     these are typed fields per the migration, not polymorphic Hash
#     lookups. Each becomes its own typed ivar (@title, @body, etc.)
#     in the emitted target, enabling fully-typed output for Rust and
#     similar strict targets without forcing an `untyped`/`Any` escape.
#   - has_many :comments, dependent: :destroy: explicit `comments`
#     getter returning a CollectionProxy, plus explicit `destroy`
#     override that cascades.
#   - validates: explicit `validate` instance method calling validates_*
#     helpers provided by the runtime.
#   - broadcasts_to ->(_article) { "articles" }, inserts_by: :prepend:
#     explicit overrides of lifecycle hooks (after_create_commit etc.)
#     with the lambda body inlined.

class Article < ApplicationRecord
  def self.table_name
    "articles"
  end

  # Schema columns (from articles.id/title/body/created_at/updated_at).
  # attr_accessor generates typed-ivar getter/setter pairs; Base tracks
  # the names via schema_column_names to drive adapter (de)serialization.
  attr_accessor :id, :title, :body, :created_at, :updated_at

  # --- has_many :comments ---
  def comments
    if @_comments
      @_comments
    else
      @_comments = ActiveRecord::CollectionProxy.new(
        owner: self,
        target_class: Comment,
        foreign_key: :article_id
      )
    end
  end

  # --- dependent: :destroy ---
  def destroy
    comments.each(&:destroy)
    super
  end

  # --- validates :title, presence: true ---
  # --- validates :body,  presence: true, length: { minimum: 10 } ---
  def validate
    validates_presence_of(:title)
    validates_presence_of(:body)
    validates_length_of(:body, minimum: 10)
  end

  # --- broadcasts_to ->(_article) { "articles" }, inserts_by: :prepend ---
  def after_create_commit
    broadcast_prepend_to("articles")
  end

  def after_update_commit
    broadcast_replace_to("articles")
  end

  def after_destroy_commit
    broadcast_remove_to("articles")
  end
end
