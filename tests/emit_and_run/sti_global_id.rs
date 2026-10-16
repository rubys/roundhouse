//! STI GlobalIDs on the emitted CRuby tree. Rows hydrate base-classed,
//! so the name a row mints and the name a locate accepts both come from
//! the `type` column, never from the instance's Ruby class.

use super::emit_and_run;

/// A subclass row mints the same GlobalID however it was loaded, a
/// base-class `only:` lookup finds it under that name, and a subclass
/// name paired with a plain row's id finds nothing (Rails' scoped
/// `Articles::Featured.find` would not find it either).
#[test]
fn sti_rows_mint_and_locate_by_their_type_column() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.string \"title\"\n    t.text \"body\"\n",
            "    t.string \"title\"\n    t.string \"type\"\n    t.text \"body\"\n",
        )
        .write("app/models/articles/featured.rb", "class Articles::Featured < Article\nend\n")
        .write(
            "app/models/article_locator.rb",
            "class ArticleLocator\n  def self.find_by_gid(gid)\n    GlobalID::Locator.locate gid, only: Article\n  end\nend\n",
        )
        .write(
            "test/models/sti_global_id_test.rb",
            r#"require "test_helper"

class StiGlobalIdTest < ActiveSupport::TestCase
  test "sti rows mint and locate by their type column" do
    body = "A sufficiently long article body."
    plain = Article.create!(title: "Plain", body: body)
    featured = Articles::Featured.create!(title: "Featured", body: body)

    loaded = Article.find(featured.id)
    assert_equal featured.to_gid_param, loaded.to_gid_param
    refute_equal Article.find(plain.id).to_gid_param, loaded.to_gid_param

    assert_equal featured.id, ArticleLocator.find_by_gid(loaded.to_gid_param).id
    assert_equal plain.id, ArticleLocator.find_by_gid(plain.to_gid_param).id

    forged = plain.becomes!(Articles::Featured).to_gid_param
    assert_nil ArticleLocator.find_by_gid(forged)
  end
end
"#,
        )
        .run_test("test/models/sti_global_id_test.rb")
        .assert_passes();
}
