# Transpiled shape of fixtures/real-blog/app/models/article.rb.
#
# Expansion applied (per ruby2js's filter/rails/model.rb):
#   - attributes: explicit getter/setter per schema column
#   - has_many :comments, dependent: :destroy: explicit `comments` getter
#     returning a CollectionProxy, plus explicit `destroy` override that
#     cascades.
#   - validates: explicit `validate` instance method calling validates_*
#     helper methods provided by the runtime.
#   - broadcasts_to ->(_article) { "articles" }, inserts_by: :prepend:
#     explicit overrides of lifecycle hooks (after_create_commit etc.)
#     with the lambda body inlined and the inserts_by: routed to the
#     matching broadcast_*_to runtime call.

class Article < ApplicationRecord
  def self.table_name
    "articles"
  end

  # --- Attributes (from schema: articles.id/title/body/created_at/updated_at) ---
  def id;             @attributes[:id];             end
  def id=(v);         @attributes[:id] = v;         end
  def title;          @attributes[:title];          end
  def title=(v);      @attributes[:title] = v;      end
  def body;           @attributes[:body];           end
  def body=(v);       @attributes[:body] = v;       end
  def created_at;     @attributes[:created_at];     end
  def created_at=(v); @attributes[:created_at] = v; end
  def updated_at;     @attributes[:updated_at];     end
  def updated_at=(v); @attributes[:updated_at] = v; end

  # --- has_many :comments ---
  def comments
    @_comments ||= ActiveRecord::CollectionProxy.new(
      owner: self,
      target_class: Comment,
      foreign_key: :article_id
    )
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
