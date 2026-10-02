//! `.pdf.erb` / `.csv.erb` / `.txt.erb` templates beside an
//! `.html.erb` sibling (#F3) — Procore has 141 `.pdf.erb`, 85
//! `.csv.erb`, and 2 `.txt.erb` templates that `walk_erb` used to skip
//! outright (`view template not ingested: erb (non-html format)`).
//! They are ordinary ERB producing text (a PDF renderer's HTML input,
//! a `CSV.generate` body, a mailer's plaintext part), so they join the
//! same allow-list `.svg.erb` / `.json.erb` already ride, and lower
//! the same way: `<action>_<format>`, beside the html template.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::view::lowers_through_view_path;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/reports_controller.rb",
            "class ReportsController < ApplicationController\n  def show\n  end\nend\n",
        ),
        ("app/views/reports/show.html.erb", "<h1><%= @title %></h1>\n"),
        (
            "app/views/reports/show.pdf.erb",
            "<h1><%= @title %></h1>\n<p><%= @body %></p>\n",
        ),
        (
            "app/views/reports/show.csv.erb",
            "<%= CSV.generate do |csv|\n  csv << [@title]\nend.html_safe -%>\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn pdf_and_csv_templates_ingest_beside_the_html_template() {
    let app = app();
    let names: Vec<(String, String)> =
        app.views.iter().map(|v| (v.name.as_str().to_string(), v.format.as_str().to_string())).collect();
    assert_eq!(names.len(), 3, "expected html + pdf + csv views: {names:?}");
    assert!(names.contains(&("reports/show".to_string(), "html".to_string())), "{names:?}");
    assert!(names.contains(&("reports/show".to_string(), "pdf".to_string())), "{names:?}");
    assert!(names.contains(&("reports/show".to_string(), "csv".to_string())), "{names:?}");

    // Every one of them is a live, emitted view (not analysis-only —
    // that's specifically the `.text.erb` mailer-variant carve-out).
    for view in &app.views {
        assert!(
            lowers_through_view_path(view),
            "{}.{} should lower through the view path",
            view.name.as_str(),
            view.format.as_str()
        );
    }
}

#[test]
fn txt_template_ingests_and_lowers_beside_the_html_template() {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/views/notifications/notice.txt.erb",
            "Hello <%= @name %>\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");

    let txt = app
        .views
        .iter()
        .find(|v| v.name.as_str() == "notifications/notice" && v.format.as_str() == "txt")
        .expect("notice.txt.erb ingested");
    assert!(lowers_through_view_path(txt), "txt format should lower through the view path");
    // `txt` is a distinct format from Rails' `text` mailer-variant
    // spelling, which stays analysis-only; confirm the two don't
    // collide on that flag.
    assert!(!txt.analysis_only, "txt (unlike text) is a fully emitted format");
}
