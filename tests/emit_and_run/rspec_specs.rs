use super::emit_and_run;

// RSpec's verdict on CRuby is the expectation: the spec is written to pass
// unchanged under `rspec` in the generated blog (5 examples). The
// emitted tests must pass on the same app, and fail where RSpec fails.
const SPEC: &str = r#"require "rails_helper"

RSpec.describe Article, type: :model do
  let(:title) { "Getting Started" }
  let(:article) { Article.new(title: title, body: "A body that is long enough.") }

  it "is valid with a title and a long body" do
    expect(article).to be_valid
    expect(article.title).to eq("Getting Started")
    expect(described_class).to eq(Article)
  end

  context "without title" do
    let(:title) { nil }

    before { article.validate }

    it "is invalid" do
      expect(article).not_to be_valid
      expect(article.errors[:title]).to include("can't be blank")
    end
  end

  context "with short body" do
    subject { Article.new(title: "Valid Title", body: "Short") }

    it { is_expected.not_to be_valid }
  end

  describe "let" do
    let(:loaded) do
      @loads = (@loads || 0) + 1
      nil
    end

    it "evaluates once per example, nil included" do
      loaded
      loaded
      expect(@loads).to eq(1)
      expect(loaded).to be_nil
    end
  end

  describe "let!" do
    let!(:saved) { Article.create!(title: "Saved", body: "Saved before the example.") }

    it "runs before the example" do
      expect(Article.where(title: "Saved").count).to eq(1)
      expect(saved).to be_persisted
    end
  end
end
"#;

const CONFIG: &str = "test_paths:\n  - spec/models\n";

fn app(spec: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("roundhouse.yml", CONFIG)
        .write("spec/models/article_spec.rb", spec)
}

// One emitted test file per example group that owns examples, named by
// the spec file and the group.
const FILES: [(&str, usize); 5] = [
    ("models_article_spec", 1),
    ("models_article_without_title_spec", 1),
    ("models_article_with_short_body_spec", 1),
    ("models_article_let_spec", 1),
    ("models_article_let_spec2", 1),
];

#[test]
fn rspec_examples_run_as_emitted_ruby_tests() {
    for (stem, _) in FILES {
        app(SPEC).run_test(&format!("test/models/{stem}_test.rb")).assert_passes();
    }
}

// A false expectation must fail the emitted test: a reader that dropped
// expectations would pass everything above.
#[test]
fn a_failing_expectation_fails_the_emitted_test() {
    let failing = SPEC.replace(
        "expect(article.title).to eq(\"Getting Started\")",
        "expect(article.title).to eq(\"Something else\")",
    );
    let run = app(&failing).run_test("test/models/models_article_spec_test.rb");
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    assert!(!run.success, "a false `eq` passed:\n{}\n{}", run.stdout, run.stderr);
}

// Two files can give one module name (`article/comments_spec.rb` and
// `article_comments_spec.rb` are both `ModelsArticleComments`). Each
// keeps its own test file: the second is numbered, and its failing
// example fails it alone.
#[test]
fn spec_files_with_the_same_module_name_keep_their_own_tests() {
    let example = |value: &str| {
        format!("require \"rails_helper\"\n\nRSpec.describe \"comments\" do\n  it \"x\" do\n    expect(1).to eq({value})\n  end\nend\n")
    };
    let overlay = || {
        emit_and_run::real_blog()
            .write("roundhouse.yml", CONFIG)
            .write("spec/models/article/comments_spec.rb", &example("1"))
            .write("spec/models/article_comments_spec.rb", &example("2"))
    };
    let first = overlay().run_test("test/models/models_article_comments_spec_test.rb");
    let second = overlay().run_test("test/models/models_article_comments_spec2_test.rb");
    assert!(first.errors.is_empty() && second.errors.is_empty(), "{:?} {:?}", first.errors, second.errors);
    assert!(first.success != second.success, "one file passes and the other fails:\n{}\n{}", first.stdout, second.stdout);
}

// The point of the lane: the same examples run inside the Spinel build.
#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn rspec_examples_pass_in_the_spinel_build() {
    let (emitted, errors) = app(SPEC).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let files: Vec<String> = FILES.iter().map(|(stem, _)| format!("test/{stem}_test.rb")).collect();
    for (file, (stem, n)) in files.iter().zip(FILES) {
        let expected = std::fs::read_to_string(emitted.join(format!("{file}.expected")))
            .unwrap_or_else(|e| panic!("{file}.expected in {}: {e}", emitted.display()));
        let class = stem
            .split('_')
            .map(|w| w[..1].to_uppercase() + &w[1..])
            .collect::<String>();
        assert_eq!(expected, format!("{class}: {n} tests passed\n"), "{file}");
    }
    let output = std::process::Command::new("spin")
        .arg("test")
        .args(&files)
        .current_dir(&*emitted)
        .output()
        .expect("spawn spin");
    std::fs::write(emitted.join("spin-test.stdout"), &output.stdout).unwrap();
    std::fs::write(emitted.join("spin-test.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "spin test failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// `type: :request` groups are integration tests: `get` drives the app and
// `response` is the one it answered, with a failing control.
const REQUEST_SPEC: &str = r#"require "rails_helper"

RSpec.describe "Articles", type: :request do
  it "lists the articles" do
    get "/articles"
    expect(response.status).to eq(200)
  end
end
"#;

fn request_app(spec: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("roundhouse.yml", "test_paths:\n  - spec/requests\n")
        .write("spec/requests/articles_spec.rb", spec)
}

#[test]
fn request_specs_run_as_integration_tests() {
    request_app(REQUEST_SPEC).run_test("test/models/requests_articles_spec_test.rb").assert_passes();
}

#[test]
fn a_request_spec_with_the_wrong_status_fails() {
    let run = request_app(&REQUEST_SPEC.replace("eq(200)", "eq(404)")).run_test("test/models/requests_articles_spec_test.rb");
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    assert!(!run.success, "a wrong status passed:\n{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("FAIL RequestsArticlesSpec#test_lists_the_articles: assertion failed"), "failed for another reason:\n{}\n{}", run.stdout, run.stderr);
}
