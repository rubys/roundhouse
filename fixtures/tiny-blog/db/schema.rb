ActiveRecord::Schema.define do
  create_table "posts", force: :cascade do |t|
    t.string "title", null: false
  end
  create_table "comments", force: :cascade do |t|
    t.text "body", null: false
    t.bigint "post_id", null: false
  end
end
