//! Kaminari's `page` / `per` and the paginator readers on the runtime
//! Relation (#311).
//!
//! The catalog typed `page` / `per` as relation builders, but the runtime
//! Relation defined neither, nor the readers a paged relation answers
//! (`current_page`, `total_pages`, …), so `check` reported every reader
//! as a failed dispatch and the emitted program raised NoMethodError on
//! `page`. The page size comes from the app's `Kaminari.configure` block
//! when it has one, else Kaminari's default of 25.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONTROLLER: &str = r#"class ReportsController < ApplicationController
  def index
    reports = Report.order(:title).page(params[:page]).per(params[:per])
    render plain: "page=#{reports.current_page} per=#{reports.limit_value} total=#{reports.total_count} " \
                  "pages=#{reports.total_pages} next=#{reports.next_page.inspect} prev=#{reports.prev_page.inspect} " \
                  "first=#{reports.first_page?} last=#{reports.last_page?} out=#{reports.out_of_range?} " \
                  "titles=#{reports.map(&:title).join(",")}"
  end

  def unordered
    reports = Report.page(params[:page])
    render plain: "page=#{reports.current_page} per=#{reports.limit_value} titles=#{reports.map(&:title).join(",")}"
  end
end
"#;

const INITIALIZER: &str = "Kaminari.configure do |config|\n  config.default_per_page = 2\nend\n";

/// A small app paginating `Report`s, with or without the initializer.
fn app(initializer: Option<&str>) -> emit_and_run::Overlay {
    let overlay = emit_and_run::empty_app()
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"kaminari\"\n")
        .write(
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Archive\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("app/controllers/reports_controller.rb", CONTROLLER)
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        // The scope is there so the scope-chain pass runs and
        // `Report.order(...)` is re-rooted onto a Relation.
        .write(
            "app/models/report.rb",
            "class Report < ApplicationRecord\n  scope :titled, -> { where.not(title: nil) }\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :reports, only: :index\n  get \"unordered\", to: \"reports#unordered\"\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"reports\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        );
    match initializer {
        Some(source) => overlay.write("config/initializers/kaminari_config.rb", source),
        None => overlay,
    }
}

/// Five rows inserted out of order, and a `get` that answers the body.
const PRELUDE: &str = r#"%w[e d c b a].each { |t| Report.create(title: t) }
def get(path, query)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new(""))
  raise "GET #{path}?#{query} answered #{status}: #{body.join}" unless status == 200
  body.join
end
def expect(path, query, want)
  got = get(path, query)
  raise "GET #{path}?#{query}\n got: #{got}\nwant: #{want}" unless got == want
end
"#;

/// What Rails with kaminari answers for the same requests and rows.
#[test]
fn page_and_per_answer_what_kaminari_does() {
    let script = format!(
        r#"{PRELUDE}
expect("/reports", "page=2&per=2", "page=2 per=2 total=5 pages=3 next=3 prev=1 first=false last=false out=false titles=c,d")
expect("/reports", "page=2&per=1", "page=2 per=1 total=5 pages=5 next=3 prev=1 first=false last=false out=false titles=b")
expect("/reports", "page=3", "page=3 per=2 total=5 pages=3 next=nil prev=2 first=false last=true out=false titles=e")
expect("/reports", "", "page=1 per=2 total=5 pages=3 next=2 prev=nil first=true last=false out=false titles=a,b")
expect("/reports", "page=2&per=", "page=2 per=2 total=5 pages=3 next=3 prev=1 first=false last=false out=false titles=c,d")
expect("/reports", "page=9", "page=9 per=2 total=5 pages=3 next=nil prev=nil first=false last=false out=true titles=")
expect("/unordered", "page=2", "page=2 per=2 titles=c,b")
"#
    );
    app(Some(INITIALIZER)).run_ruby(&script).assert_passes();
}

#[test]
fn without_an_initializer_a_page_is_kaminaris_default_25_rows() {
    let script = format!(
        r#"{PRELUDE}
raise "default per page is #{{Rails.application.kaminari_default_per_page}}" unless Rails.application.kaminari_default_per_page == 25
expect("/unordered", "", "page=1 per=25 titles=e,d,c,b,a")
expect("/reports", "page=2", "page=2 per=25 total=5 pages=1 next=nil prev=nil first=false last=false out=true titles=")
"#
    );
    app(None).run_ruby(&script).assert_passes();
}

/// Only the `Kaminari.configure` block's own parameter sets the page
/// size: a `config.default_per_page =` in another config block of
/// the same initializer belongs to that block's receiver, not Kaminari.
#[test]
fn only_the_kaminari_configure_block_sets_the_page_size() {
    let initializer = "Kaminari.configure { |k| k.default_per_page = 2 }\n\nRails.application.configure do |config|\n  config.default_per_page = 50\nend\n";
    let script = format!(
        r#"{PRELUDE}
raise "default per page is #{{Rails.application.kaminari_default_per_page}}" unless Rails.application.kaminari_default_per_page == 2
expect("/reports", "page=2", "page=2 per=2 total=5 pages=3 next=3 prev=1 first=false last=false out=false titles=c,d")
"#
    );
    app(Some(initializer)).run_ruby(&script).assert_passes();
}

/// A page size the initializer computes (an ENV read) cannot be carried
/// into the emitted app, so survey mode ledgers it instead of the app
/// silently paging at Kaminari's default.
#[test]
fn a_computed_page_size_is_a_survey_gap() {
    use roundhouse::ingest::{IngestError, ingest_app_from_tree, survey};

    let tree = [
        (
            "config/application.rb",
            "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Archive\n  class Application < Rails::Application\n  end\nend\n",
        ),
        (
            "config/initializers/kaminari_config.rb",
            "Kaminari.configure do |config|\n  config.default_per_page = ENV.fetch(\"PAGE_SIZE\").to_i\nend\n",
        ),
    ]
    .into_iter()
    .map(|(path, source)| (path.into(), source.as_bytes().to_vec()))
    .collect();
    survey::activate();
    let result = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    result.expect("a computed page size must not abort ingest");
    assert!(
        gaps.iter().any(|gap| matches!(
            gap,
            IngestError::Unsupported { file, message }
                if file.ends_with("config/initializers/kaminari_config.rb")
                    && message.contains("default_per_page")
                    && message.contains("ENV.fetch(\"PAGE_SIZE\").to_i")
        )),
        "{gaps:?}"
    );
}
