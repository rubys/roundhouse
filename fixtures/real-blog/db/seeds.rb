# Sample articles and comments for the real-blog fixture.
# Seeds are idempotent — skip if data already exists. Matches
# the demo-blog shape so the E2E acceptance scenario has
# realistic fixture data to start from.
if Article.count == 0
  article1 = Article.create!(
    title: "Getting Started with Rails",
    body: "Rails is a web application framework running on the Ruby programming language. It makes building web apps faster and easier with conventions over configuration."
  )

  article1.comments.create!(
    commenter: "Alice",
    body: "Great introduction! Rails really does make development faster."
  )

  article1.comments.create!(
    commenter: "Bob",
    body: "I love how Rails handles database migrations automatically."
  )

  article2 = Article.create!(
    title: "Understanding MVC Architecture",
    body: "MVC stands for Model-View-Controller. Models handle data and business logic, Views display information to users, and Controllers coordinate between them."
  )

  article2.comments.create!(
    commenter: "Carol",
    body: "This pattern really helps keep code organized!"
  )

  Article.create!(
    title: "Roundhouse: Rails Everywhere",
    body: "Roundhouse transpiles Ruby to multiple languages, enabling Rails applications to run across browsers, servers, and edge runtimes."
  )
end
