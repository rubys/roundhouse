//! Emitted-program regressions for `Hash#to_query` and
//! `presence_in(<Array[String]>)` on the public read path. Both used to
//! stop the build with "Object extension not supported (all targets)";
//! removing that error is a claim the emitted program runs, so each is
//! pinned by an overlay on real-blog that the emitted CRuby tree boots.
//! The grammar itself is pinned byte-for-byte against activesupport in
//! tests/active_support_to_query.rs.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const INDEX: &str = "    @articles = Article.includes(:comments).order(created_at: :desc)\n";

/// EngineeredAt's 301-to-the-canonical-query shape. `params[...]` values
/// make the hash non-scalar-typed, so the call cannot take the
/// insertion-order scalar path, and the expected Location shows the
/// pairs SORTED and `+`-escaped.
#[test]
fn a_redirect_built_from_hash_to_query_has_rails_exact_location() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            INDEX,
            &format!(
                "    if params[:canon] == \"1\"\n      query = {{ \"sort\" => params[:sort], \"page\" => params[:page], \"note\" => \"a b&c\" }}.to_query\n      redirect_to \"/articles?#{{query}}\", status: :moved_permanently\n      return\n    end\n{INDEX}"
            ),
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "  test \"should get new\" do",
            "  test \"canonical redirect orders and escapes its query\" do\n    get articles_url, params: { canon: \"1\", sort: \"top\", page: \"2\" }\n    assert_response :moved_permanently\n    assert_equal \"/articles?note=a+b%26c&page=2&sort=top\", response.location.sub(\"http://www.example.com\", \"\")\n  end\n\n  test \"should get new\" do",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `params[:direction].presence_in(%w[asc desc]) || default`: a listed
/// value passes through, anything else (and a missing param) falls to
/// the default. `"sideways"` and `nil` distinguish a real allow-list from
/// one that returns its receiver or its default unconditionally.
#[test]
fn presence_in_a_literal_list_filters_a_param() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            INDEX,
            &format!(
                "    if params[:probe] == \"1\"\n      render plain: (params[:direction].presence_in(%w[asc desc]) || \"desc\")\n      return\n    end\n{INDEX}"
            ),
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "  test \"should get new\" do",
            "  test \"direction is allow-listed\" do\n    get articles_url, params: { probe: \"1\", direction: \"asc\" }\n    assert_equal \"asc\", response.body\n    get articles_url, params: { probe: \"1\", direction: \"sideways\" }\n    assert_equal \"desc\", response.body\n    get articles_url, params: { probe: \"1\" }\n    assert_equal \"desc\", response.body\n  end\n\n  test \"should get new\" do",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}
