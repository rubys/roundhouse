//! Pay gem first slice: `rescue Pay::Error` / `Pay::ActionRequired` and
//! `is_a?(Pay::Charge)` resolve. ActiveRecord APIs on Pay::* stay refused.

use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, Severity};
use roundhouse::ingest::ingest_app;

fn fixture(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "roundhouse-pay-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for (path, source) in [
        (
            "Gemfile.lock",
            "PATH\n  remote: .\n  specs:\n    app (0.1.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    pay (12.1.1)\n\nDEPENDENCIES\n  pay\n",
        ),
        (
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  post \"/checkout\", to: \"checkouts#create\"\n  get \"/return\", to: \"returns#show\"\nend\n",
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/models/article.rb",
            "class Article < ApplicationRecord\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/checkouts_controller.rb",
            "class CheckoutsController < ApplicationController\n  def create\n    raise Pay::ActionRequired, \"confirm\"\n  rescue Pay::ActionRequired => e\n    render plain: e.message\n  rescue Pay::Error => e\n    render plain: e.message\n  end\nend\n",
        ),
        (
            "app/controllers/returns_controller.rb",
            "class ReturnsController < ApplicationController\n  def show\n    object = Article.first\n    if object.is_a?(Pay::Charge)\n      head :ok\n    elsif object.is_a?(Pay::Subscription)\n      head :ok\n    else\n      head :not_found\n    end\n  end\nend\n",
        ),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    root
}

#[test]
fn pay_exception_and_is_a_constants_check_clean() {
    let root = fixture("clean");
    let mut app = ingest_app(&root).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let diags = diagnose(&app);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    assert!(
        errors.is_empty(),
        "Pay::Error / ActionRequired / Charge / Subscription must resolve:\n{errors:#?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pay_charge_all_stays_refused() {
    let root = fixture("all");
    std::fs::write(
        root.join("app/controllers/dashboards_controller.rb"),
        "class DashboardsController < ApplicationController\n  def show\n    @charges = Pay::Charge.all\n  end\nend\n",
    )
    .unwrap();
    let routes = "Rails.application.routes.draw do\n  get \"/dashboard\", to: \"dashboards#show\"\nend\n";
    std::fs::write(root.join("config/routes.rb"), routes).unwrap();

    // Drive the product path (analyze + unknown-gem attribution), not
    // diagnose-only: attribution used to quiet `.all` into a note
    // because `pay` stays Unknown while Charge is catalogued for is_a?.
    let mut app = ingest_app(&root).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let mut diags = diagnose(&app);
    roundhouse::analyze::attribution::attribute_unknown_gems(&mut diags, &app);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{}: {}", d.code(), d.message))
        .collect();
    let notes: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .map(|d| format!("{}: {}", d.code(), d.message))
        .collect();
    assert!(
        errors.iter().any(|e| e.contains("`all`") && e.contains("Pay::Charge")),
        "Pay::Charge.all must remain an error after attribution (not AR yet):\nerrors={errors:#?}\nnotes={notes:#?}"
    );
    assert!(
        !notes.iter().any(|n| n.contains("`all`") && n.contains("Pay::Charge")),
        "Pay::Charge.all must not be downgraded to an unknown-gem note:\n{notes:#?}"
    );
    std::fs::remove_dir_all(root).ok();
}
