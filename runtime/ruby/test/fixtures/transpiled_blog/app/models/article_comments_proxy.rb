# Generated proxy for Article#comments (specialized form of has_many).
#
# Specialization expansion (replaces ActiveRecord::CollectionProxy[T]):
# the target class, table, and foreign key are baked in as concrete
# values — no generic type parameter, no constructor arguments beyond
# the owner. Each method returns/yields concrete `Comment` instances,
# letting strict targets (Rust, Crystal) emit fully-typed output
# without needing type-variable substitution in the IR.

class ArticleCommentsProxy
  include Enumerable

  def initialize(owner)
    @owner = owner
  end

  def to_a
    rows = ActiveRecord.adapter.where(:comments, article_id: @owner.id)
    rows.map { |r| Comment.instantiate(r) }
  end

  # `each(&block)` would be more idiomatic, but the ingest doesn't yet
  # support `&local` block forwarding (only `&:symbol`). Yield form
  # is semantically equivalent for a single-arity Enumerable each;
  # revisit when the ingest gap closes.
  def each
    to_a.each { |comment| yield comment }
  end

  def size
    ActiveRecord.adapter.where(:comments, article_id: @owner.id).size
  end
  alias_method :length, :size
  alias_method :count, :size

  def empty?
    size == 0
  end

  def build(attrs = {})
    Comment.new(attrs.merge(article_id: @owner.id))
  end

  def create(attrs = {})
    record = build(attrs)
    record.save
    record
  end
end
