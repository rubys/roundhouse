# What tests/render_json_full_messages.rs checks:
#
#     ruby tests/render_json_full_messages/oracle.rb
# activerecord 8.1.4: `render json:` of full_messages, bare and in a Hash.
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define { create_table(:articles) { |t| t.string :title; t.text :body } }
class Article < ActiveRecord::Base
  validates :title, presence: true
  validates :body, presence: true, length: { minimum: 10 }
end
article = Article.new(title: "", body: "x")
puts [article.valid?, article.errors.full_messages].to_json
puts({ errors: article.errors.full_messages }.to_json)
puts({ errors: article.errors }.to_json)
