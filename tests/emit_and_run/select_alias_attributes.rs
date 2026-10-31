use super::emit_and_run;

const ARTICLE_ASSOC: &str = "  has_many :comments, dependent: :destroy\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            ARTICLE_ASSOC,
            concat!(
                "  has_many :comments, dependent: :destroy\n",
                "  has_many :commenter_tallies,\n",
                "    -> { group(:commenter).select(\"count(*) as times, commenter\").order(times: :desc) },\n",
                "    class_name: \"Comment\"\n",
            ),
        )
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article\n",
            concat!(
                "  belongs_to :article\n\n",
                "  attribute :total, :integer\n\n",
                "  def self.totals\n",
                "    select(\"article_id, count(1) AS total\").group(:article_id).order(:article_id)\n",
                "  end\n\n",
                "  def self.longest_body(article)\n",
                "    Comment.where(article_id: article.id).select(\"max(length(body)) as \\\"widest\\\"\").to_a.first.widest\n",
                "  end\n",
            ),
        )
        .edit(
            "app/models/application_record.rb",
            "  primary_abstract_class\n",
            "  primary_abstract_class\n\n  def rank\n    \"parent\"\n  end\n",
        )
        .write(
            "app/views/articles/scores.html.erb",
            "<% Comment.select(\"id, 1 AS score, 2 AS rank\").each do |c| %><%= c.score %><%= c.rank %><% end %>\n",
        )
}

const ASSERTIONS: &str = r##"article = Article.create!(title: "Tallies", body: "a body that is long enough")
other = Article.create!(title: "Other", body: "a body that is long enough")
["alice", "alice", "bob"].each { |name| Comment.create!(article: article, commenter: name, body: "hi there") }
plain = Comment.create!(article: other, commenter: "carol", body: "a longer body")
tallies = article.commenter_tallies.map { |t| [t.times, t.commenter] }
raise tallies.inspect unless tallies == [[2, "alice"], [1, "bob"]]
totals = Comment.totals.map { |c| [c.article_id, c.total] }
raise totals.inspect unless totals == [[article.id, 3], [other.id, 1]]
widest = Comment.longest_body(other)
raise widest.inspect unless widest == 13
missing = begin
  Comment.find(plain.id).times
  "answered"
rescue NoMethodError => e
  e.message
end
raise missing unless missing == "undefined method 'times' for an instance of Comment"
raise Comment.find(plain.id).total.inspect unless Comment.find(plain.id).total.nil?
scored = Comment.select("id, 1 AS score").to_a.first.score
raise scored.inspect unless scored == 1
ranked = Comment.select("id, 2 AS rank").to_a.first.rank
raise ranked.inspect unless ranked == "parent"
puts "select aliases passed"
"##;

#[test]
fn select_aliases_read_back_on_the_loaded_records() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("select aliases passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn select_aliases_read_back_on_spinel() {
    let run = app().run_spinel_with_db(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("select aliases passed"), "{}", run.stdout);
}

#[test]
fn a_target_without_the_runtime_relation_reports_each_alias() {
    let (_emitted, errors) = app().emit(roundhouse::project::BuildTarget::Typescript);
    for alias in ["times", "total", "widest", "score"] {
        assert!(
            errors.iter().any(|e| e.contains(&format!("select alias `{alias}` not supported (typescript)"))),
            "{alias}: {errors:?}"
        );
    }
    assert!(!errors.iter().any(|e| e.contains("select alias `rank`")), "{errors:?}");
}
