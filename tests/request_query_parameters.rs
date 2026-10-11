//! `request.query_parameters` on the public read path.
//!
//! EngineeredAt's `redirect_noncanonical_query_parameters` strips
//! tracking keys from the query string and 301s to the cleaned URL.
//! `request.query_parameters` was registered `Untyped` and the Ruby
//! family's `Request` had no reader, so the action passed `check` and
//! would have raised NoMethodError at run time. This pins the typed
//! reader by overlaying the app's action verbatim on real-blog and
//! asserting the Location Rails produces. The expectations were
//! produced by running the same controller code under Rails 8.1 (see
//! the package report), not derived from the implementation.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const BEFORE: &str = "  before_action :set_article, only: %i[ show edit update destroy ]\n";

const REDIRECT: &str = r##"  before_action :redirect_noncanonical_query_parameters

  TRACKING_QUERY_PARAMS = %w[
    dclid
    fbclid
    gclid
    ref
  ].freeze

  TRACKING_QUERY_PARAM_PREFIXES = %w[
    utm_
  ].freeze

  def redirect_noncanonical_query_parameters
    return unless request.get? || request.head?

    cleaned_params = request.query_parameters.deep_dup
    changed = remove_tracking_query_params(cleaned_params)

    if cleaned_params["page"].to_s == "1" && cleaned_params.except("page").empty?
      cleaned_params.delete("page")
      changed = true
    end

    return unless changed

    query = cleaned_params.to_query
    redirect_to "#{request.path}#{query.present? ? "?#{query}" : ""}", status: :moved_permanently
  end

  def remove_tracking_query_params(query_params)
    changed = false

    query_params.keys.each do |key|
      next unless tracking_query_param?(key)

      query_params.delete(key)
      changed = true
    end

    changed
  end

  def tracking_query_param?(key)
    TRACKING_QUERY_PARAMS.include?(key) ||
      TRACKING_QUERY_PARAM_PREFIXES.any? { |prefix| key.start_with?(prefix) }
  end
"##;

const TESTS: &str = r##"  def location_path
    response.location.sub("http://www.example.com", "")
  end

  test "tracking keys are stripped and the rest kept" do
    get "/articles?utm_source=x&sort=top"
    assert_response :moved_permanently
    assert_equal "/articles?sort=top", location_path
  end

  test "page=1 alone is dropped" do
    get "/articles?page=1"
    assert_response :moved_permanently
    assert_equal "/articles", location_path
  end

  test "page=1 beside another key stays, and a clean query does not redirect" do
    get "/articles?page=1&sort=top"
    assert_response :success
    get "/articles?sort=top"
    assert_response :success
  end

  test "a nested key survives the redirect" do
    get "/articles?gclid=1&a[b]=1&a[c]=2&ref=z&tags[]=x&tags[]=y"
    assert_response :moved_permanently
    assert_equal "/articles?a%5Bb%5D=1&a%5Bc%5D=2&tags%5B%5D=x&tags%5B%5D=y", location_path
  end

  test "prefix match is on the key, not the value" do
    get "/articles?q=utm_source&utm_medium=m"
    assert_response :moved_permanently
    assert_equal "/articles?q=utm_source", location_path
  end

"##;

#[test]
fn the_canonical_query_redirect_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            BEFORE,
            &format!("{BEFORE}{REDIRECT}"),
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "  test \"should get new\" do",
            &format!("{TESTS}  test \"should get new\" do"),
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}
