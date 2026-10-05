//! Emitted-program regression for this fix (kept out of tests/emit_and_run.rs
//! so concurrent appends there do not conflict). Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// `owner.<has_many>.new(attrs)` — CollectionProxy's alias for `build`
/// (`@project.tasks.new(task_params)`, or a blank nested form's
/// `project.tasks.new`). Only
/// `build`/`create`/`create!` were seeded with the foreign key; `new`
/// reached the reader's Array as `Array#new`.
#[test]
fn an_association_new_builds_with_the_foreign_key() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true",
            "  def draft_comment(who)\n    comments.new(commenter: who, body: \"draft\")\n  end\n\n  validates :title, presence: true",
        )
        .edit(
            "app/views/articles/show.html.erb",
            "<% content_for :title, \"Showing article\" %>",
            "<% content_for :title, \"Showing article\" %>\n<% blank = @article.comments.new %><i id=\"blank\"><%= blank.article_id %>/<%= blank.persisted? %></i>",
        )
        .run_ruby(r#"
def get(path)
  out = StringIO.new
  Main.run({ "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "HTTP_ACCEPT" => "text/html" }, StringIO.new(""), out)
  out.string
end
Article.delete_all
a = Article.create!(title: "Owner", body: "Long enough body")
c = a.draft_comment("amy")
raise "assoc.new: #{c.inspect}" unless c.is_a?(Comment) && c.article_id == a.id && !c.persisted?
show = get("/articles/#{a.id}")
raise "view assoc.new:\n#{show}" unless show.include?("<i id=\"blank\">#{a.id}/false</i>")
puts "assoc new"
"#)
        .assert_passes();
}
