//! Non-scalar permitted keys reach the model (#1338). activerecord /
//! actionpack 8.1.4 (oracles `nested_permit.rb`, `nested_permit2.rb`):
//! `permit(…, comments_attributes: [:commenter, :body])` and
//! `expect(article: […, comments_attributes: [[:commenter, :body]]])`
//! keep the nested value, an Array of hashes or a hash of hashes
//! (`{"0" => {…}}`), each hash cut to the listed keys, and
//! `Article.new(permitted)` hands it to `comments_attributes=`.
//!
//! `permit(…, labels: [])` keeps an Array of scalars, and drops one
//! holding a hash or an array, a scalar, or a hash (`nested_permit2.rb`);
//! the ruby family carries it as an `Array[String]`.
//!
//! On the ruby family the key was dropped from the synthesized params
//! record with a "non-scalar" warning, so the writer never ran. Strict
//! targets keep that warning.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const WRITER: &str = "  has_many :comments, dependent: :destroy\n\n  def comments_attributes=(list)\n    entries = list.is_a?(Hash) ? list.values : list\n    @pending_comments = []\n    entries.each { |entry| @pending_comments << entry }\n  end\n\n  after_save :save_pending_comments\n\n  def save_pending_comments\n    pending = @pending_comments\n    return if pending.nil?\n    pending.each { |entry| Comment.create!(article_id: id, commenter: entry.keys.sort.join(\"+\"), body: entry[\"body\"]) }\n    @pending_comments = []\n  end\n";

const TEST: &str = r#"
  test "nested comments as an array" do
    post articles_url, params: { article: { title: "Nested", body: "A sufficiently long body for validation.",
      comments_attributes: [{ commenter: "c1", body: "one", evil: "x" }, { commenter: "c2", body: "two" }] } }
    comments = Comment.where(article_id: Article.last.id).order(:id)
    assert_equal ["one", "two"], comments.map(&:body)
    assert_equal ["body+commenter", "body+commenter"], comments.map(&:commenter)
  end

  test "nested comments as a hash of hashes" do
    post articles_url, params: { article: { title: "Nested", body: "A sufficiently long body for validation.",
      comments_attributes: { "0" => { commenter: "c1", body: "one" }, "1" => { commenter: "c2", body: "two", evil: "x" } } } }
    comments = Comment.where(article_id: Article.last.id).order(:id)
    assert_equal ["one", "two"], comments.map(&:body)
    assert_equal ["body+commenter", "body+commenter"], comments.map(&:commenter)
  end

  test "an update carries them too" do
    article = Article.create!(title: "Nested", body: "A sufficiently long body for validation.")
    patch article_url(article), params: { article: { comments_attributes: [{ commenter: "c", body: "later" }] } }
    assert_equal ["later"], Comment.where(article_id: article.id).map(&:body)
  end

"#;

fn app(helper: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", WRITER)
        .edit("app/controllers/articles_controller.rb", "      params.expect(article: [ :title, :body ])\n", helper)
        .edit("test/controllers/articles_controller_test.rb", "  test \"should get new\" do\n", &format!("{TEST}  test \"should get new\" do\n"))
}

const EXPECT: &str = "      params.expect(article: [ :title, :body, comments_attributes: [ [ :commenter, :body ] ] ])\n";
const PERMIT: &str = "      params.require(:article).permit(:title, :body, comments_attributes: [ :commenter, :body ])\n";

#[test]
fn expect_list_reaches_the_writer_on_ruby() {
    app(EXPECT).run_test("test/controllers/articles_controller_test.rb").assert_passes();
}

#[test]
fn permit_list_reaches_the_writer_on_ruby() {
    app(PERMIT).run_test("test/controllers/articles_controller_test.rb").assert_passes();
}

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
list = [{ "commenter" => "c1", "body" => "one", "evil" => "x" }, { "commenter" => "c2", "body" => "two" }]
raw = { "article" => { "title" => "Nested", "body" => "A sufficiently long body for validation.", "comments_attributes" => list } }
a = Article.from_params(ArticleParams.from_raw(raw))
check("saved", a.save, true)
comments = Comment.where(article_id: a.id).order(:id)
check("an array", comments.map(&:body), ["one", "two"])
check("filtered", comments.map(&:commenter), ["body+commenter", "body+commenter"])
numbered = { "0" => { "commenter" => "c1", "body" => "three" }, "x" => { "commenter" => "c", "body" => "skipped" } }
raw = { "article" => { "title" => "Nested", "body" => "A sufficiently long body for validation.", "comments_attributes" => numbered } }
b = Article.from_params(ArticleParams.from_raw(raw))
check("saved", b.save, true)
check("a hash of hashes", Comment.where(article_id: b.id).map(&:body), ["three"])
puts "nested carry passed"
"##;

#[test]
fn from_raw_carries_the_list_on_ruby() {
    let run = app(EXPECT).run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested carry passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn from_raw_carries_the_list_on_spinel() {
    let run = app(EXPECT).run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nested carry passed"), "{}", run.stdout);
}

const LABELS_WRITER: &str = "  has_many :comments, dependent: :destroy\n\n  def labels=(list)\n    self.body = body.to_s + \" [\" + list.join(\",\") + \"]\"\n  end\n";
const LABELS_PERMIT: &str = "      params.require(:article).permit(:title, :body, labels: [])\n";

const LABELS_TEST: &str = r#"
  test "a scalar list" do
    post articles_url, params: { article: { title: "Labels", body: "A sufficiently long body for validation.", labels: ["a", "b"] } }
    assert_equal "A sufficiently long body for validation. [a,b]", Article.last.body
  end

  test "a list holding a hash is dropped" do
    post articles_url, params: { article: { title: "Labels", body: "A sufficiently long body for validation.", labels: ["a", { x: "1" }] } }
    assert_equal "A sufficiently long body for validation.", Article.last.body
  end

"#;

fn labels_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", LABELS_WRITER)
        .edit("app/controllers/articles_controller.rb", "      params.expect(article: [ :title, :body ])\n", LABELS_PERMIT)
        .edit("test/controllers/articles_controller_test.rb", "  test \"should get new\" do\n", &format!("{LABELS_TEST}  test \"should get new\" do\n"))
}

#[test]
fn a_scalar_list_reaches_the_writer_on_ruby() {
    labels_app().run_test("test/controllers/articles_controller_test.rb").assert_passes();
}

const LABELS_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
raw = { "article" => { "title" => "Labels", "body" => "A sufficiently long body for validation.", "labels" => ["a", "b"] } }
a = Article.from_params(ArticleParams.from_raw(raw))
check("a scalar list", a.body, "A sufficiently long body for validation. [a,b]")
raw = { "article" => { "title" => "Labels", "body" => "A sufficiently long body for validation.", "labels" => "a" } }
b = Article.from_params(ArticleParams.from_raw(raw))
check("a scalar is dropped", b.body, "A sufficiently long body for validation.")
puts "labels passed"
"##;

#[test]
fn from_raw_carries_a_scalar_list_on_ruby() {
    let run = labels_app().run_ruby(LABELS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("labels passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn from_raw_carries_a_scalar_list_on_spinel() {
    let run = labels_app().run_spinel_with_db(LABELS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("labels passed"), "{}", run.stdout);
}

// Two permit lists on one resource that differ only by a nested key are
// two mass-assignment boundaries (oracle `nested_permit3.rb`: a narrower
// `permit(:title, :body)` drops `comments_attributes` as unpermitted).
// They must be two specs, not one carrying the union.
const DRAFTS: &str = "class DraftsController < ApplicationController\n  def create\n    @article = Article.new(article_params)\n    if @article.save\n      head :created\n    else\n      head :unprocessable_content\n    end\n  end\n\n  private\n\n  def article_params\n    params.expect(article: [ :title, :body ])\n  end\nend\n";

const BOUNDARY_TEST: &str = r#"
  test "the narrow controller does not carry the wide one's nested key" do
    assert_difference("Article.count") do
      post drafts_url, params: { article: { title: "Draft", body: "A sufficiently long body for validation.",
        comments_attributes: [{ commenter: "c1", body: "one" }] } }
    end
    assert_equal 0, Comment.where(article_id: Article.last.id).count
  end

  test "the wide controller still carries it" do
    post articles_url, params: { article: { title: "Wide", body: "A sufficiently long body for validation.",
      comments_attributes: [{ commenter: "c1", body: "one" }] } }
    assert_equal ["one"], Comment.where(article_id: Article.last.id).map(&:body)
  end

"#;

fn boundary_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", WRITER)
        .edit("app/controllers/articles_controller.rb", "      params.expect(article: [ :title, :body ])\n", EXPECT)
        .write("app/controllers/drafts_controller.rb", DRAFTS)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  resources :drafts, only: [:create]\n")
        .edit("test/controllers/articles_controller_test.rb", "  test \"should get new\" do\n", &format!("{BOUNDARY_TEST}  test \"should get new\" do\n"))
}

#[test]
fn a_narrower_permit_on_the_same_resource_does_not_carry_the_key_on_ruby() {
    boundary_app().run_test("test/controllers/articles_controller_test.rb").assert_passes();
}

const BOUNDARY_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
raw = { "article" => { "title" => "T", "body" => "A sufficiently long body for validation.", "comments_attributes" => [{ "commenter" => "c", "body" => "one" }] } }
narrow = Article.from_drafts_article_params(DraftsArticleParams.from_raw(raw))
check("narrow saved", narrow.save, true)
check("narrow carries no comments", Comment.where(article_id: narrow.id).count, 0)
wide = Article.from_params(ArticleParams.from_raw(raw))
check("wide saved", wide.save, true)
check("wide carries them", Comment.where(article_id: wide.id).map(&:body), ["one"])
puts "boundary passed"
"##;

#[test]
fn the_narrow_record_reads_no_nested_key_on_ruby() {
    let run = boundary_app().run_ruby(BOUNDARY_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("boundary passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn the_narrow_record_reads_no_nested_key_on_spinel() {
    let run = boundary_app().run_spinel_with_db(BOUNDARY_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("boundary passed"), "{}", run.stdout);
}

/// One controller whose two helpers differ only by a nested key gets two
/// records, each carrying its own key alone.
#[test]
fn two_helpers_differing_only_by_a_nested_key_get_two_records() {
    let root = std::env::temp_dir().join(format!("roundhouse-params-nested-two-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let file = source.join("app/controllers/articles_controller.rb");
    let text = std::fs::read_to_string(&file).unwrap()
        .replacen("    @article = Article.new(article_params)\n", "    @article = Article.new(intake_params)\n", 1)
        .replacen("      if @article.update(article_params)\n", "      if @article.update(revise_params)\n", 1)
        .replacen(
            "    def article_params\n      params.expect(article: [ :title, :body ])\n    end\n",
            "    def intake_params\n      params.expect(article: [ :title, :body, comments_attributes: [ [ :commenter, :body ] ] ])\n    end\n\n    def revise_params\n      params.expect(article: [ :title, :body, labels: [] ])\n    end\n",
            1,
        );
    assert!(text.contains("def revise_params"), "{text}");
    std::fs::write(&file, text).unwrap();
    let model = source.join("app/models/article.rb");
    let text = std::fs::read_to_string(&model).unwrap().replacen(
        "  has_many :comments, dependent: :destroy\n",
        "  has_many :comments, dependent: :destroy\n\n  def comments_attributes=(list)\n    @c = list\n  end\n\n  def labels=(list)\n    @l = list\n  end\n",
        1,
    );
    std::fs::write(&model, text).unwrap();
    let out = root.join("out");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "--allow-unsupported", "-o"])
        .arg(&out)
        .arg(&source)
        .output()
        .expect("run roundhouse");
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    let records: Vec<String> = std::fs::read_dir(out.join("app/models"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("article") && e.file_name().to_string_lossy().ends_with("params.rb"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    std::fs::remove_dir_all(&root).ok();
    let with = |k: &str| records.iter().filter(|r| r.contains(&format!("\"{k}\""))).count();
    assert_eq!(records.len(), 2, "two article records");
    assert_eq!(with("comments_attributes"), 1, "one record reads comments_attributes");
    assert_eq!(with("labels"), 1, "one record reads labels");
    assert!(
        !records.iter().any(|r| r.contains("\"comments_attributes\"") && r.contains("\"labels\"")),
        "no record carries both"
    );
}

// `settings: {}` carries any hash as submitted; a test request's hashes
// arrive String-keyed at every depth, as a form's do.
const SETTINGS_WRITER: &str = "  has_many :comments, dependent: :destroy\n\n  def settings=(value)\n    self.body = body.to_s + \" \" + value[\"matrix\"][0][0].keys.map { |key| key.class.name + \":\" + key.to_s }.join(\",\")\n  end\n";

const SETTINGS_TEST: &str = r#"
  test "nested arrays arrive String-keyed" do
    post articles_url, params: { article: { title: "Deep", body: "A sufficiently long body for validation.",
      settings: { matrix: [[{ flag: "on" }]] } } }
    assert_equal "A sufficiently long body for validation. String:flag", Article.last.body
  end

"#;

#[test]
fn hashes_in_nested_arrays_arrive_string_keyed_on_ruby() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", SETTINGS_WRITER)
        .edit("app/controllers/articles_controller.rb", "      params.expect(article: [ :title, :body ])\n", "      params.require(:article).permit(:title, :body, settings: {})\n")
        .edit("test/controllers/articles_controller_test.rb", "  test \"should get new\" do\n", &format!("{SETTINGS_TEST}  test \"should get new\" do\n"))
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

// `expect(article: [… , k: [:a]])` filters `k` as one hash (oracle
// `nested_permit2.rb`: "hash" mode). The controller rewrite turns the
// `expect` into a `permit` marked `FROM_PARAMS_EXPECT` before the helper
// is replaced, and the spec must still be found there.
const EXPECT_HASH: &str = "      params.expect(article: [ :title, :body, settings: [ :theme ] ])\n";
const THEME_WRITER: &str = "  has_many :comments, dependent: :destroy\n\n  def settings=(value)\n    raise \"unfiltered key reached writer\" if value.key?(\"extra\")\n    self.body = body.to_s + \" theme=\" + value[\"theme\"].to_s\n  end\n";

const EXPECT_HASH_TEST: &str = r#"
  test "an expect hash key reaches the writer" do
    post articles_url, params: { article: { title: "Theme", body: "A sufficiently long body for validation.",
      settings: { theme: "dark", extra: "x" } } }
    assert_equal "A sufficiently long body for validation. theme=dark", Article.last.body
  end

"#;

#[test]
fn an_expect_hash_key_reaches_the_writer_on_ruby() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", THEME_WRITER)
        .edit("app/controllers/articles_controller.rb", "      params.expect(article: [ :title, :body ])\n", EXPECT_HASH)
        .edit("test/controllers/articles_controller_test.rb", "  test \"should get new\" do\n", &format!("{EXPECT_HASH_TEST}  test \"should get new\" do\n"))
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// A strict target's params record still holds Strings only: the key
/// stays dropped there, with its warning; the ruby family carries it.
#[test]
fn a_strict_target_keeps_the_warning() {
    let root = std::env::temp_dir().join(format!("roundhouse-params-nested-carry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let edit = |path: &str, find: &str, replace: &str| {
        let file = source.join(path);
        let text = std::fs::read_to_string(&file).unwrap().replacen(find, replace, 1);
        std::fs::write(file, text).unwrap();
    };
    edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", WRITER);
    edit("app/controllers/articles_controller.rb", "      params.expect(article: [ :title, :body ])\n", EXPECT);
    let run = |target: &str| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", target, "--allow-unsupported", "-o"])
            .arg(root.join(format!("out-{target}")))
            .arg(&source)
            .output()
            .expect("run roundhouse");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let rust = run("rust");
    let ruby = run("ruby");
    std::fs::remove_dir_all(&root).ok();
    let dropped = "permitted key `comments_attributes` on `article`";
    assert!(rust.contains(dropped), "{rust}");
    assert!(!ruby.contains(dropped), "{ruby}");
}
