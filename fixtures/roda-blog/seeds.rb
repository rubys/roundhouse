# Idempotent sample data. Run with:  bundle exec ruby seeds.rb
require_relative "app"

Comment.dataset.delete
Article.dataset.delete

first = Article.create(
  title: "Hello Roda",
  body: "The first post on this Roda + Sequel blog. Welcome aboard!"
)
Article.create(
  title: "Why Sequel transpiles cleanly",
  body: "Sequel builds SQL from Ruby objects rather than string fragments, " \
        "which makes it a tidy target for whole-program analysis."
)

first.add_comment(commenter: "Ada",   body: "Great to see this working end to end.")
first.add_comment(commenter: "Grace", body: "Nested routes and everything.")

puts "Seeded #{Article.count} articles and #{Comment.count} comments."
