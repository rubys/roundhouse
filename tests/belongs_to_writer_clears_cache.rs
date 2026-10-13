//! A belongs_to reader on the Ruby family memoizes its record. Assigning
//! the association afterwards must not answer from that cache: in Rails,
//! `comment.article = nil` makes `comment.article` nil, and assigning a
//! different owner makes the reader answer that owner. The writer used to
//! change the foreign key alone, so the reader kept returning the owner
//! it had loaded first.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn integer_key_writer_clears_the_memoized_owner() {
    emit_and_run::real_blog()
        .run_ruby(
            r#"
first = Article.create!(title: "First", body: "A sufficiently long body")
second = Article.create!(title: "Second", body: "Another sufficiently long body")
c = Comment.create!(article_id: first.id, commenter: "Widget", body: "hi")

c = Comment.find(c.id)
raise "reads its owner" unless c.article.title == "First"
c.article = second
raise "a different owner reads #{c.article&.title.inspect}" unless c.article && c.article.title == "Second"
raise "the key follows the owner" unless c.article_id == second.id
c.article = nil
raise "nil writer still reads #{c.article&.title.inspect}" unless c.article.nil?
c.article = first
raise "re-assigned owner reads #{c.article&.title.inspect}" unless c.article && c.article.title == "First"
c.article_id = second.id
raise "a direct key write reads #{c.article&.title.inspect}" unless c.article.title == "Second"
fresh = Article.new(title: "Fresh", body: "A body long enough to be valid")
c.article = fresh
raise "an unsaved owner is the one read back" unless c.article.equal?(fresh)
c.save!
raise "the unsaved owner is saved with the comment" unless fresh.id.to_i > 0 && Comment.find(c.id).article_id == fresh.id
dropped = Article.new(title: "Dropped", body: "A body long enough to be valid")
c.article = dropped
c.article = nil
raise "a comment whose owner was set to nil saved" if c.save
raise "the dropped unsaved owner was autosaved" if Article.where(title: "Dropped").count > 0
puts "INTEGER OK"
"#,
        )
        .assert_passes();
}

#[test]
fn uuid_key_writer_clears_the_memoized_owner() {
    // The reader memoizes once the app's code mentions `includes` (the
    // preload pass adds the cache guard), which real-blog does and this
    // fixture does not; its controller also names a filter nothing
    // defines, which is not what this test is about.
    emit_and_run::tiny_blog_uuid()
        .edit(
            "app/models/comment.rb",
            "  def self.counts_by_post",
            "  def self.with_posts\n    Comment.includes(:post).to_a\n  end\n\n  def self.counts_by_post",
        )
        .edit("app/controllers/posts_controller.rb", "  before_action :authenticate_user\n", "")
        .run_ruby(
            r#"
first = Post.new(title: "first")
first.save
second = Post.new(title: "second")
second.save
c = Comment.new(body: "hi")
c.post = first
c.save

c = Comment.find(c.id)
raise "reads its owner" unless c.post.title == "first"
c.post = second
raise "a different owner reads #{c.post&.title.inspect}" unless c.post && c.post.title == "second"
raise "the key follows the owner" unless c.post_id == second.id
c.post = nil
raise "nil writer still reads #{c.post&.title.inspect}" unless c.post.nil?
c.post = first
raise "re-assigned owner reads #{c.post&.title.inspect}" unless c.post && c.post.title == "first"
puts "UUID OK"
"#,
        )
        .assert_passes();
}
