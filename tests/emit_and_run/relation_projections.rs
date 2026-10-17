use super::emit_and_run;

#[test]
fn emitted_relation_projects_rows_and_picks_the_first_row() {
    emit_and_run::real_blog()
        .write(
            "app/models/projection_probe.rb",
            r#"class ProjectionProbe
  def self.rows
    Article.where(title: "First").pluck(:id, :title)
  end

  def self.first_row
    Article.where(title: "First").pick(:id, :title)
  end

  def self.mapping
    Article.where(title: "First").pluck(:title, :id).to_h
  end
end
"#,
        )
        .run_ruby(
            r#"
article = Article.new
article.title = "First"
article.body = "A body long enough."
article.save!
raise "rows differ" unless ProjectionProbe.rows == [[article.id, "First"]]
raise "first row differs" unless ProjectionProbe.first_row == [article.id, "First"]
raise "mapping differs" unless ProjectionProbe.mapping == { "First" => article.id }
relation = ActiveRecord::Relation.new(Article).order(:id).limit(2)
raise "single column differs" unless relation.pluck(:title) == ["First"]
raise "duplicate columns differ" unless relation.pluck(:id, :id) == [[article.id, article.id]]
raise "projection retained" unless relation.to_a.first.title == "First"
empty = ActiveRecord::Relation.new(Article).where(id: -1)
raise "empty pluck differs" unless empty.pluck(:id, :title) == []
raise "empty pick differs" unless empty.pick(:id, :title).nil?
"#,
        )
        .assert_passes();
}
