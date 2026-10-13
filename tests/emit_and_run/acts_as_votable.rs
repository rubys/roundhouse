//! `acts_as_votable` on real-blog: `ActsAsVotable::Vote` reads and the
//! `liked_by` / `unliked_by` / `vote_by` write path with cached columns.
//! Expected values are what the real gem 0.14.0 produced on ActiveRecord 8.1
//! + sqlite for the same script (work/_scratch/votable-oracle/oracle.rb).

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "Gemfile.lock",
            "    debug (1.11.1)\n",
            "    acts_as_votable (0.14.0)\n      activerecord (>= 4.2)\n    debug (1.11.1)\n",
        )
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|\n",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"cached_votes_total\", default: 0, null: false\n    t.integer \"cached_votes_up\", default: 0, null: false\n    t.integer \"cached_scoped_flag_votes_up\", default: 0, null: false\n",
        )
        .edit(
            "db/schema.rb",
            "create_table \"comments\", force: :cascade do |t|\n",
            "create_table \"readers\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"type\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"votes\", force: :cascade do |t|\n    t.string \"votable_type\"\n    t.bigint \"votable_id\"\n    t.string \"voter_type\"\n    t.bigint \"voter_id\"\n    t.boolean \"vote_flag\"\n    t.string \"vote_scope\"\n    t.integer \"vote_weight\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n    t.index [\"votable_type\", \"votable_id\"], name: \"index_votes_on_votable\"\n    t.index [\"voter_type\", \"voter_id\"], name: \"index_votes_on_voter\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|\n",
        )
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  acts_as_votable\n  has_many :comments, dependent: :destroy\n",
        )
        .write("app/models/admin.rb", "class Admin < Reader\nend\n")
        .write("app/models/reader.rb", "class Reader < ApplicationRecord\n  acts_as_voter\nend\n")
        .write(
            "app/queries/upvoted.rb",
            "class Upvoted\n  def self.article_ids(reader)\n    ActsAsVotable::Vote.where(voter: reader, vote_flag: true, votable_type: \"Article\").pluck(:votable_id)\n  end\n\n  def self.flag_join\n    ActsAsVotable::Vote.sanitize_sql_array([\"INNER JOIN votes ON votes.votable_type = ? AND votes.vote_scope = ?\", \"Article\", \"flag\"])\n  end\nend\n",
        )
}

const TEST: &str = r#"require "test_helper"

class VotesTest < ActiveSupport::TestCase
  def setup
    @a1 = articles(:one)
    @a2 = articles(:two)
    @r1 = Reader.create!(name: "r1")
    @r2 = Reader.create!(name: "r2")
  end

  test "liked_by creates one vote, repeats do not duplicate, caches follow" do
    assert_equal true, @a1.liked_by(@r1)
    assert_equal true, @a1.vote_registered?
    assert_equal 1, ActsAsVotable::Vote.count
    @a1.reload
    assert_equal 1, @a1.cached_votes_up
    assert_equal 1, @a1.cached_votes_total
    vote = ActsAsVotable::Vote.last
    assert_equal true, vote.vote_flag
    assert_equal 1, vote.vote_weight
    assert_nil vote.vote_scope

    assert_equal true, @a1.liked_by(@r1)
    assert_equal false, @a1.vote_registered?
    assert_equal 1, ActsAsVotable::Vote.count
  end

  test "a scoped vote is separate from the default scope and counts into the totals" do
    @a1.liked_by(@r1)
    assert_equal true, @a1.vote_by(voter: @r1, vote_scope: "flag")
    assert_equal 2, ActsAsVotable::Vote.count
    @a1.reload
    assert_equal 2, @a1.cached_votes_up
    assert_equal 1, @a1.cached_scoped_flag_votes_up
    assert_equal 2, @a1.cached_votes_total
  end

  test "a changed mind updates the same row; unliked_by removes only its own scope" do
    @a1.liked_by(@r1)
    @a1.vote_by(voter: @r1, vote_scope: "flag")
    assert_equal true, @a1.disliked_by(@r2)
    assert_equal 3, ActsAsVotable::Vote.count
    @a1.reload
    assert_equal 2, @a1.cached_votes_up
    assert_equal 3, @a1.cached_votes_total
    assert_equal false, ActsAsVotable::Vote.where(voter_id: @r2.id).last.vote_flag

    assert_equal true, @a1.liked_by(@r2)
    assert_equal 3, ActsAsVotable::Vote.count
    assert_equal 3, @a1.reload.cached_votes_up
    assert_equal true, ActsAsVotable::Vote.where(voter_id: @r2.id).last.vote_flag

    assert_equal true, @a1.unliked_by(@r1)
    assert_equal 2, ActsAsVotable::Vote.count
    @a1.reload
    assert_equal 2, @a1.cached_votes_up
    assert_equal 1, @a1.cached_scoped_flag_votes_up
    assert_equal 1, ActsAsVotable::Vote.where(votable: @a1, voter_id: @r1.id, vote_scope: "flag").count
    assert_equal false, @a1.vote_registered?
  end

  test "reads: where(voter:) pluck, voter predicates, finders" do
    @a1.liked_by(@r1)
    @a1.vote_by(voter: @r1, vote_scope: "flag")
    @a2.liked_by(@r1)
    @a1.liked_by(@r2)
    assert_equal [@a1.id, @a1.id, @a2.id].sort, Upvoted.article_ids(@r1).sort
    assert_equal [@a1.id], Upvoted.article_ids(@r2)
    assert_equal [], ActsAsVotable::Vote.where(voter: @r2, vote_flag: false, votable_type: "Article").pluck(:votable_id)
    assert_includes Upvoted.flag_join, "votes.vote_scope = 'flag'"

    assert_equal true, @r1.voted_up_on?(@a2)
    assert_equal true, @r1.voted_up_on?(@a1)
    assert_equal true, @r1.voted_up_on?(@a1, vote_scope: "flag")
    assert_equal false, @r2.voted_up_on?(@a2)
    assert_equal true, @r1.liked?(@a2)
    assert_equal false, @r2.voted_down_on?(@a1)
    assert_equal true, @a1.voted_on_by?(@r2)
    assert_equal true, @a1.voted_up_by?(@r1)
    assert_equal 3, @a1.get_up_votes.count
    assert_equal 0, @a1.get_down_votes.count
    assert_equal 1, @a1.get_up_votes(vote_scope: "flag").count
    assert_equal 3, @a1.count_votes_up(true)
    assert_equal 3, @a1.votes_for.count
    assert_equal 3, @r1.votes.count
  end

  test "an STI subclass voter is stored and found under its base class" do
    admin = Admin.create!(name: "root")
    @a1.liked_by(admin)
    @a1.liked_by(@r1)
    assert_equal "Reader", ActsAsVotable::Vote.where(voter_id: admin.id, vote_scope: nil).last.voter_type
    assert_equal 2, ActsAsVotable::Vote.where(votable: @a1).count
    assert_equal 1, ActsAsVotable::Vote.where(voter: admin).count
    assert_equal 1, ActsAsVotable::Vote.where(voter: @r1).count
    assert_equal [@a1.id], ActsAsVotable::Vote.where(voter: admin, vote_flag: true, votable_type: "Article").pluck(:votable_id)
    assert_equal true, admin.voted_up_on?(@a1)
    assert_equal true, @a1.voted_up_by?(admin)
    assert_equal 1, admin.votes.count
    assert_equal true, @a1.unliked_by(admin)
    assert_equal 0, ActsAsVotable::Vote.where(voter: admin).count
    assert_equal 1, ActsAsVotable::Vote.where(voter: @r1).count
    # a record of a class that is not a voter finds no rows, not "any type"
    assert_equal 0, ActsAsVotable::Vote.where(voter: @a1).count
  end

  test "a nil voter condition is IS NULL on both columns, like the plain belongs_to path" do
    orphan = ActsAsVotable::Vote.new(vote_scope: "orphan", vote_flag: true)
    orphan.save(validate: false)
    @a1.liked_by(@r1)
    assert_equal 1, ActsAsVotable::Vote.where(voter: nil, vote_scope: "orphan").count
    assert_equal 0, ActsAsVotable::Vote.where(voter: nil, vote_scope: nil).count
    # Rails: nil (literal or a variable) is `voter_id IS NULL` with no type condition
    nobody = nil
    assert_equal 1, ActsAsVotable::Vote.where(voter: nobody, vote_scope: "orphan").count
    assert_equal 1, ActsAsVotable::Vote.where(voter: nobody).count
    assert_equal 0, ActsAsVotable::Vote.where(voter: nobody, vote_scope: nil).count
    assert_equal 1, ActsAsVotable::Vote.where(voter: @r1).count
  end

  test "weight, meaning of words, duplicate, nil voter, scopes, dependent delete" do
    @a2.liked_by(@r1)
    assert_equal true, @a2.vote_by(voter: @r2, vote: "down", vote_weight: 3)
    vote = ActsAsVotable::Vote.where(votable: @a2, voter_id: @r2.id).last
    assert_equal 3, vote.vote_weight
    assert_equal false, vote.vote_flag
    @a2.reload
    assert_equal 1, @a2.cached_votes_up
    assert_equal 2, @a2.cached_votes_total

    assert_equal true, @a2.vote_by(voter: @r2, vote: true, duplicate: true)
    assert_equal 2, ActsAsVotable::Vote.where(votable: @a2, voter_id: @r2.id).count
    assert_equal false, @a2.vote_by(voter: nil)

    assert_equal 2, ActsAsVotable::Vote.up.count
    assert_equal 1, ActsAsVotable::Vote.down.count
    assert_equal 3, ActsAsVotable::Vote.for_type(Article).count
    assert_equal 3, ActsAsVotable::Vote.by_type(Reader).count
    assert_equal "Article", ActsAsVotable::Vote.first.votable.class.name
    assert_equal "Reader", ActsAsVotable::Vote.first.voter.class.name

    @a2.destroy
    assert_equal 0, ActsAsVotable::Vote.where(votable_id: @a2.id, votable_type: "Article").count
  end
end
"#;

#[test]
fn acts_as_votable_runs_in_emitted_ruby() {
    let run = app().write("test/models/votes_test.rb", TEST).run_test("test/models/votes_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("7 tests passed"), "{}", run.stdout);
}
