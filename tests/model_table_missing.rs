//! PostgreSQL: model table missing, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

use std::collections::HashMap;
use std::path::PathBuf;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_concrete_model_without_a_schema_table_is_reported() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/structure.sql", "CREATE TABLE widgets (\n    id bigint NOT NULL,\n    type character varying\n);\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/models/gadget.rb", "class Gadget < Widget\nend\n"),
        ("app/models/gizmo.rb", "class Gizmo < ApplicationRecord\nend\n"),
        ("app/models/analytics_record.rb", "class AnalyticsRecord < ApplicationRecord\n  self.abstract_class = true\n  connects_to database: { writing: :analytics }\nend\n"),
        ("app/models/report.rb", "class Report < AnalyticsRecord\nend\n"),
        ("app/models/search.rb", "class Search\n  include ActiveModel::Validations\n  validates :q, presence: true\nend\n"),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<String> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.contains("model_table_not_in_schema"))
        .collect();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("`Gizmo` reads table `gizmos`")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("`Report` reads table `reports` on database `analytics`,")), "{errors:?}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_model_on_a_mysql_configuration_is_out_of_target() {
    let yml = "mysql_config: &mysql_config\n  adapter: trilogy\n\ntest_side: &test_side\n  database: side_test\n  <<: *mysql_config\n\ntest:\n  primary:\n    adapter: postgresql\n    database: app_test\n  side:\n    <<: *<%= \"test_side\" %>\n  other:\n    adapter: postgresql\n    database: app_test\n";
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("roundhouse.yml", ""),
        ("config/database.yml", yml),
        ("db/structure.sql", "CREATE TABLE moves (\n    id bigint NOT NULL\n);\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/side_record.rb", "class SideRecord < ApplicationRecord\n  self.abstract_class = true\n  connects_to database: { writing: :side }\nend\n"),
        ("app/models/other_record.rb", "class OtherRecord < ApplicationRecord\n  self.abstract_class = true\n  connects_to database: { writing: :other }\nend\n"),
        ("app/models/move.rb", "class Move < SideRecord\nend\n"),
        ("app/models/setting.rb", "class Setting < SideRecord\nend\n"),
        ("app/models/report.rb", "class Report < OtherRecord\nend\n"),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let rows: Vec<String> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.contains("model_on_non_pg_database") || d.contains("model_table_not_in_schema"))
        .collect();
    let non_pg: Vec<&String> = rows.iter().filter(|d| d.contains("model_on_non_pg_database")).collect();
    assert_eq!(non_pg.len(), 2, "{rows:?}");
    assert!(non_pg.iter().any(|e| e.contains("`Move` reads table `moves` on database `side`, which `config/database.yml` gives adapter `trilogy`")), "{rows:?}");
    assert!(non_pg.iter().any(|e| e.contains("`Setting` reads table `settings` on database `side`")), "{rows:?}");
    assert!(rows.iter().any(|e| e.contains("model_table_not_in_schema") && e.contains("`Report` reads table `reports` on database `other`,")), "{rows:?}");
}
