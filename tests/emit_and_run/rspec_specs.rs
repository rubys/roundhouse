use super::emit_and_run;

// RSpec's verdict on CRuby is the expectation: the spec is written to pass
// unchanged under `rspec` in the generated blog (8 + 3 examples). The
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

  describe "block expectations" do
    it "raise errors" do
      expect { Article.find(-1) }.to raise_error(ActiveRecord::RecordNotFound)
      expect { raise ArgumentError, "bad value" }.to raise_error(ArgumentError, "bad value")
      expect { raise ArgumentError, "bad value" }.to raise_error(/bad/)
      expect { article.valid? }.not_to raise_error
    end

    it "change values" do
      expect { Article.create!(title: "New", body: "A body that is long enough.") }.to change(Article, :count).by(1)
      expect { article.title = "Renamed" }.to change { article.title }.from("Getting Started").to("Renamed")
      expect { article.valid? }.not_to change { Article.count }
    end
  end

  describe "helper configuration" do
    it "runs the helper's hooks and includes its modules" do
      expect(@configured).to eq(true)
      expect(article_title_for(article)).to eq("Getting Started")
    end
  end
end
"#;

// What `rails generate rspec:install` writes, trimmed, plus a hook and an
// included support module.
const RAILS_HELPER: &str = r#"require "spec_helper"
ENV["RAILS_ENV"] ||= "test"
require_relative "../config/environment"
abort("The Rails environment is running in production mode!") if Rails.env.production?
require "rspec/rails"
Dir[Rails.root.join("spec", "support", "**", "*.rb")].sort.each { |f| require f }

RSpec.configure do |config|
  config.use_transactional_fixtures = true
  config.infer_spec_type_from_file_location!
  config.filter_rails_from_backtrace!
  config.before { @configured = true }
  config.include ArticleHelpers
end
"#;

const SPEC_HELPER: &str = r#"RSpec.configure do |config|
  config.expect_with :rspec do |expectations|
    expectations.include_chain_clauses_in_custom_matcher_descriptions = true
  end
  config.shared_context_metadata_behavior = :apply_to_host_groups
end
"#;

const SUPPORT: &str = "module ArticleHelpers\n  def article_title_for(article)\n    article.title\n  end\nend\n";

// A request spec against the blog's scaffold controller.
const RESPONSE_SPEC: &str = r#"require "rails_helper"

RSpec.describe "Articles", type: :request do
  let!(:article) { Article.create!(title: "Request spec", body: "A body that is long enough.") }

  it "lists articles" do
    get articles_path
    expect(response).to have_http_status(:ok)
    expect(response).to be_successful
    expect(response.body).to include("Request spec")
  end

  it "creates an article and redirects to it" do
    expect {
      post articles_path, params: { article: { title: "From a spec", body: "A body that is long enough." } }
    }.to change(Article, :count).by(1)
    expect(response).to redirect_to(article_path(Article.last))
    expect(response).to have_http_status(:redirect)
    expect(response).not_to be_successful
  end

  it "rejects an invalid article" do
    post articles_path, params: { article: { title: "", body: "" } }
    expect(response).to have_http_status(:unprocessable_content)
    expect(response).not_to have_http_status(:redirect)
  end
end
"#;

const CONFIG: &str = "test_paths:\n  - spec/models\n  - spec/requests\n";

fn app(spec: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("roundhouse.yml", CONFIG)
        .write("spec/rails_helper.rb", RAILS_HELPER)
        .write("spec/spec_helper.rb", SPEC_HELPER)
        .write("spec/support/article_helpers.rb", SUPPORT)
        .write("spec/models/article_spec.rb", spec)
        .write("spec/requests/articles_spec.rb", RESPONSE_SPEC)
}

// One emitted test file per example group that owns examples, named by
// the spec file and the group.
const FILES: [(&str, usize); 8] = [
    ("models_article_spec", 1),
    ("models_article_without_title_spec", 1),
    ("models_article_with_short_body_spec", 1),
    ("models_article_let_spec", 1),
    ("models_article_let_spec2", 1),
    ("models_article_block_expectations_spec", 2),
    ("models_article_helper_configuration_spec", 1),
    ("requests_articles_spec", 3),
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

// The block expectations fail where RSpec's do.
#[test]
fn failing_block_expectations_fail_the_emitted_test() {
    for (find, replace) in [
        ("raise_error(ArgumentError, \"bad value\")", "raise_error(ArgumentError, \"other\")"),
        ("change(Article, :count).by(1)", "change(Article, :count).by(2)"),
        ("not_to change { Article.count }", "to change { Article.count }"),
    ] {
        let run = app(&SPEC.replacen(find, replace, 1)).run_test("test/models/models_article_block_expectations_spec_test.rb");
        assert!(run.errors.is_empty(), "{find}: {:?}", run.errors);
        assert!(!run.success, "`{replace}` passed:\n{}\n{}", run.stdout, run.stderr);
    }
}

// The response matchers fail where rspec-rails' do.
#[test]
fn failing_response_matchers_fail_the_emitted_test() {
    for (find, replace) in [
        ("expect(response).to have_http_status(:ok)", "expect(response).to have_http_status(:not_found)"),
        ("expect(response).not_to be_successful", "expect(response).to be_successful"),
        ("expect(response).not_to have_http_status(:redirect)", "expect(response).to have_http_status(:redirect)"),
    ] {
        let run = emit_and_run::real_blog()
            .write("roundhouse.yml", CONFIG)
            .write("spec/rails_helper.rb", RAILS_HELPER)
            .write("spec/spec_helper.rb", SPEC_HELPER)
            .write("spec/support/article_helpers.rb", SUPPORT)
            .write("spec/requests/articles_spec.rb", &RESPONSE_SPEC.replacen(find, replace, 1))
            .run_test("test/models/requests_articles_spec_test.rb");
        assert!(run.errors.is_empty(), "{find}: {:?}", run.errors);
        assert!(!run.success, "`{replace}` passed:\n{}\n{}", run.stdout, run.stderr);
    }
}

// An `after` hook runs after the example: one that raises fails it.
#[test]
fn after_hooks_run_after_the_example() {
    let spec = SPEC.replacen(
        "  describe \"let\" do\n",
        "  describe \"let\" do\n    after { raise \"after hook ran\" }\n",
        1,
    );
    let run = app(&spec).run_test("test/models/models_article_let_spec_test.rb");
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    assert!(!run.success, "the raising after hook did not fail the test:\n{}\n{}", run.stdout, run.stderr);
    assert!(format!("{}{}", run.stdout, run.stderr).contains("after hook ran"), "{}\n{}", run.stdout, run.stderr);
}

// rspec-mocks read as Mocha chains. Ruby lane only: on Spinel, a stub
// that returns a value from an app method is not served yet.
const MOCKS_SPEC: &str = r#"require "rails_helper"

RSpec.describe Article, type: :model do
  it "stubs and expects messages" do
    allow(Article).to receive(:count).and_return(42)
    expect(Article.count).to eq(42)
    expect(Article).to receive(:find_by).with(title: "Wanted").and_return(nil)
    expect(Article.find_by(title: "Wanted")).to be_nil
    expect(Article).not_to receive(:destroy_all)
    thing = double("thing", name: "Named")
    expect(thing.name).to eq("Named")
    allow_any_instance_of(Article).to receive(:valid?).and_return(false)
    expect(Article.new(title: "T", body: "A body that is long enough.").valid?).to eq(false)
  end
end
"#;

#[test]
fn rspec_mocks_run_as_mocha_on_ruby() {
    let run = app(SPEC).write("spec/models/article_mocks_spec.rb", MOCKS_SPEC).run_test("test/models/models_article_mocks_spec_test.rb");
    run.assert_passes();
    // An unmet expectation and an unexpected call fail the test.
    for (find, replace) in [
        ("expect(Article.find_by(title: \"Wanted\")).to be_nil", "expect(Article.find_by(title: \"Other\")).to be_nil"),
        ("expect(thing.name).to eq(\"Named\")", "Article.destroy_all\n    expect(thing.name).to eq(\"Named\")"),
    ] {
        let run = app(SPEC)
            .write("spec/models/article_mocks_spec.rb", &MOCKS_SPEC.replacen(find, replace, 1))
            .run_test("test/models/models_article_mocks_spec_test.rb");
        assert!(run.errors.is_empty(), "{replace}: {:?}", run.errors);
        assert!(!run.success, "`{replace}` passed:\n{}\n{}", run.stdout, run.stderr);
    }
}

// Stubs on app methods with answers (`lower::mocha`'s typed answer
// slots): a class method, and an argument-keyed answer on an object a
// `let` builds. These run inside the Spinel build too.
const GREETER: &str = r#"class Greeter
  def self.current
    Greeter.new("real")
  end

  def initialize(name)
    @name = name
  end

  def name
    @name
  end

  def greet(who)
    "hello " + who + " from " + @name
  end
end
"#;

const STUBS_SPEC: &str = r#"require "rails_helper"

RSpec.describe Greeter do
  let(:greeter) { Greeter.new("x") }

  before do
    allow(Greeter).to receive(:current).and_return(greeter)
    allow(greeter).to receive(:greet).with("ann").and_return("hi ann")
  end

  it "answers from the stubs" do
    expect(Greeter.current).to equal(greeter)
    expect(Greeter.current.greet("ann")).to eq("hi ann")
  end

  it "counts an expected call" do
    expect(greeter).to receive(:name).and_return("named")
    expect(greeter.name).to eq("named")
  end
end
"#;

fn stubs_app(spec: &str) -> emit_and_run::Overlay {
    app(SPEC).write("app/models/greeter.rb", GREETER).write("spec/models/greeter_spec.rb", spec)
}

#[test]
fn app_method_stubs_from_specs_run_on_ruby() {
    stubs_app(STUBS_SPEC).run_test("test/models/models_greeter_spec_test.rb").assert_passes();
    // A call the keyed stub does not admit, and an expected call never
    // made, each fail the test.
    for (find, replace) in [
        ("expect(Greeter.current.greet(\"ann\")).to eq(\"hi ann\")", "Greeter.current.greet(\"bob\")"),
        ("    expect(greeter.name).to eq(\"named\")\n", ""),
    ] {
        let run = stubs_app(&STUBS_SPEC.replacen(find, replace, 1)).run_test("test/models/models_greeter_spec_test.rb");
        assert!(run.errors.is_empty(), "{find}: {:?}", run.errors);
        assert!(!run.success, "`{find}` -> `{replace}` passed:\n{}\n{}", run.stdout, run.stderr);
    }
}

#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite development libraries"]
fn app_method_stubs_from_specs_pass_in_the_spinel_build() {
    let (emitted, errors) = stubs_app(STUBS_SPEC).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let output = std::process::Command::new("spin")
        .args(["test", "test/models_greeter_spec_test.rb"])
        .current_dir(&*emitted)
        .output()
        .expect("spawn spin");
    std::fs::write(emitted.join("spin-test.stdout"), &output.stdout).unwrap();
    assert!(
        output.status.success(),
        "spin test failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
