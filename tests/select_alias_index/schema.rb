ActiveRecord::Schema.define do
  create_table "articles" do |t|
    t.string "title"
  end
  create_table "comments" do |t|
    t.integer "article_id"
    t.string "body"
  end
end
