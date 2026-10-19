use super::emit_and_run;

/// Issue #985. Numericality (`only_integer`, `greater_than`,
/// `less_than`, and the inclusive pair the store ships), inclusion,
/// and format already lowered; ingest was dropping the keys, so
/// `create!` saved records Rails rejects. Each wired option is pinned
/// here, including the `validates_*_of` spelling. Symbol `if:` /
/// `unless:` use the same guard as `validate :method`. `equal_to`,
/// `other_than`, uniqueness, comparison, `length: is:`, and a lambda
/// condition stay diagnostics and are not enforced.
const MODEL: &str = r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  # Broadcast article changes to index page subscribers
  # Lambda receives the record being broadcast
  broadcasts_to ->(_article) { "articles" }, inserts_by: :prepend

  validates :title, presence: true
  validates :body, presence: true, length: { minimum: 10 }
  validates :price, numericality: { greater_than: 0, less_than: 100 }
  validates :inventory_count, numericality: { greater_than_or_equal_to: 0 }
  validates_numericality_of :ceiling, greater_than_or_equal_to: 1, less_than_or_equal_to: 10
  validates :rating, numericality: { only_integer: true, greater_than: 0 }
  validates :parity, numericality: { equal_to: 1, other_than: 2 }
  validates :color, inclusion: { in: %w[red blue] }
  validates :stock, inclusion: { in: 1..3 }
  validates :score, inclusion: { in: 1..3 }
  validates :code, format: { with: /\A[A-Z]{3}\z/ }
  validates_numericality_of :max_stock, greater_than: 0, less_than: 50
  validates_inclusion_of :kind, in: %w[one two]
  validates_format_of :sku, with: /\A\d{2}\z/
  validates :label, presence: true, if: :live?
  validates :extra, presence: true, unless: :draft?
  validates :note, presence: true, if: -> { status == "live" }
  validates :title, uniqueness: true
  validates :level, comparison: { less_than_or_equal_to: :cap }
  validates :tag, length: { maximum: 4, is: 2 }

  def live?
    status == "live"
  end

  def draft?
    status == "draft"
  end
end
"#;

const SCHEMA_FIND: &str =
    "    t.string \"title\"\n    t.text \"body\"\n    t.datetime \"created_at\", null: false\n";
const SCHEMA_REPLACE: &str = "    t.string \"title\"\n    t.text \"body\"\n    t.integer \"price\", default: 0\n    t.integer \"inventory_count\", default: 0\n    t.integer \"ceiling\", default: 1\n    t.float \"rating\"\n    t.integer \"parity\", default: 0\n    t.string \"color\"\n    t.integer \"stock\", default: 0\n    t.float \"score\"\n    t.integer \"max_stock\", default: 0\n    t.string \"code\"\n    t.string \"kind\"\n    t.string \"sku\"\n    t.string \"label\"\n    t.string \"extra\"\n    t.string \"note\"\n    t.string \"status\", default: \"draft\"\n    t.string \"tag\"\n    t.integer \"level\", default: 0\n    t.integer \"cap\", default: 0\n    t.datetime \"created_at\", null: false\n";

#[test]
fn validates_numericality_inclusion_and_format_reject_on_save() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_FIND, SCHEMA_REPLACE)
        .write("app/models/article.rb", MODEL)
        .run_ruby(r##"
base = {
  title: "Ok", body: "0123456789", price: 5, rating: 2, color: "red", stock: 2, score: 1.5,
  inventory_count: 0, ceiling: 1, parity: 9,
  code: "ABC", max_stock: 10, kind: "one", sku: "12", status: "draft",
  label: "", extra: "", note: "", tag: "toolong", level: 9, cap: 1
}

def messages(attrs)
  record = Article.new(attrs)
  [record.valid?, record.errors]
end

def refute_save(attrs, needle)
  begin
    Article.create!(attrs)
    raise "saved #{needle}: #{attrs.inspect}"
  rescue ActiveRecord::RecordInvalid => e
    raise "#{needle} message #{e.message}" unless e.message.include?(needle)
  end
end

valid, msgs = messages(base)
raise "valid base rejected: #{msgs.inspect}" unless valid

# numericality: greater_than / less_than, and validates_numericality_of.
_, price_low = messages(base.merge(price: 0))
raise "price 0: #{price_low.inspect}" unless price_low.any? { |m| m.include?("greater than 0") }
_, price_high = messages(base.merge(price: 100))
raise "price 100: #{price_high.inspect}" unless price_high.any? { |m| m.include?("less than 100") }
_, rating_frac = messages(base.merge(rating: 1.5))
raise "rating 1.5: #{rating_frac.inspect}" unless rating_frac.any? { |m| m.include?("integer") }
_, stock_cap = messages(base.merge(max_stock: 0))
raise "max_stock 0: #{stock_cap.inspect}" unless stock_cap.any? { |m| m.include?("greater than 0") }
_, stock_hi = messages(base.merge(max_stock: 50))
raise "max_stock 50: #{stock_hi.inspect}" unless stock_hi.any? { |m| m.include?("less than 50") }
refute_save(base.merge(title: "Price", price: 0), "greater than 0")
# Inclusive bounds, including validates_numericality_of. 0 is inside >= 0.
valid, msgs = messages(base.merge(title: "Zero", inventory_count: 0, ceiling: 10))
raise "inclusive edge rejected: #{msgs.inspect}" unless valid
_, stocked = messages(base.merge(inventory_count: -1))
raise "inventory -1: #{stocked.inspect}" unless stocked.any? { |m| m.include?("greater than or equal to 0") }
_, ceil_lo = messages(base.merge(ceiling: 0))
raise "ceiling 0: #{ceil_lo.inspect}" unless ceil_lo.any? { |m| m.include?("greater than or equal to 1") }
_, ceil_hi = messages(base.merge(ceiling: 11))
raise "ceiling 11: #{ceil_hi.inspect}" unless ceil_hi.any? { |m| m.include?("less than or equal to 10") }
refute_save(base.merge(title: "Neg", inventory_count: -1), "greater than or equal to 0")
refute_save(base.merge(title: "Ceil", ceiling: 11), "less than or equal to 10")
# equal_to / other_than are not lowered. The call warns and does not run.
valid, msgs = messages(base.merge(title: "Parity", parity: 2))
raise "equal_to ran: #{msgs.inspect}" unless valid

# inclusion: array and integer range, and validates_inclusion_of.
_, color = messages(base.merge(color: "green"))
raise "color: #{color.inspect}" unless color.any? { |m| m.include?("not included") }
_, stock = messages(base.merge(stock: 4))
raise "stock: #{stock.inspect}" unless stock.any? { |m| m.include?("not included") }
_, score = messages(base.merge(score: 4))
raise "score 4: #{score.inspect}" unless score.any? { |m| m.include?("not included") }
valid, msgs = messages(base.merge(title: "Covered", score: 1.5))
raise "score 1.5 is inside 1..3: #{msgs.inspect}" unless valid
_, kind = messages(base.merge(kind: "no"))
raise "kind: #{kind.inspect}" unless kind.any? { |m| m.include?("not included") }
refute_save(base.merge(title: "Color", color: "green"), "not included")

# format:, and validates_format_of.
_, code = messages(base.merge(code: "abc"))
raise "code: #{code.inspect}" unless code.any? { |m| m.include?("invalid") }
_, sku = messages(base.merge(sku: "ab"))
raise "sku: #{sku.inspect}" unless sku.any? { |m| m.include?("invalid") }
refute_save(base.merge(title: "Code", code: "abc"), "invalid")

# Symbol if: / unless: guard the check. A draft may omit label and extra.
valid, msgs = messages(base.merge(title: "Draft"))
raise "draft rejected: #{msgs.inspect}" unless valid
_, live_blank = messages(base.merge(title: "Live", status: "live"))
raise "live blank: #{live_blank.inspect}" unless live_blank.any? { |m| m.include?("Label") } && live_blank.any? { |m| m.include?("Extra") }
valid, msgs = messages(base.merge(title: "LiveOk", status: "live", label: "x", extra: "y"))
raise "live filled rejected: #{msgs.inspect}" unless valid

# Lambda if:, uniqueness:, comparison:, and length: is: are not enforced.
valid, msgs = messages(base.merge(title: "Lambda", status: "live", label: "x", extra: "y", note: ""))
raise "lambda condition ran: #{msgs.inspect}" unless valid
Article.create!(base.merge(title: "Taken", label: "x", extra: "y"))
Article.create!(base.merge(title: "Taken"))
valid, msgs = messages(base.merge(title: "Compared"))
raise "comparison or length is: ran: #{msgs.inspect}" unless valid

puts "validates options passed"
"##);
    run.assert_passes();
    assert!(
        run.stdout.contains("validates options passed"),
        "{}",
        run.stdout
    );
    let warnings = emit_and_run::real_blog()
        .edit("db/schema.rb", SCHEMA_FIND, SCHEMA_REPLACE)
        .write("app/models/article.rb", MODEL)
        .ingest_warnings();
    for needle in [
        "equal_to",
        "other_than",
        "uniqueness",
        "comparison",
        "length: is",
        "if:",
    ] {
        assert!(
            warnings.iter().any(|message| message.contains(needle)),
            "missing warning {needle} in {warnings:?}"
        );
    }
}
